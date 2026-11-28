//! XFA time fields (#8411): a time picture clause as an Acrobat time pattern, and a time of day
//! between its canonical form in the data (`HH:MM:SS`, XFA 3.3 "Canonical Format", p. 1046) and
//! that pattern.

use super::Field;
use crate::model::Ui;

/// The pattern a time field without a time picture shows and is typed in: the canonical form.
pub(crate) const CANONICAL_TIME: &str = "HH:MM:ss";

/// The Acrobat time pattern (`AFTime_FormatEx` letters: `h`/`hh` and `H`/`HH` hours, `M`/`MM`
/// minutes, `s`/`ss` seconds, `tt` AM or PM) of an XFA time picture clause such as
/// `time{h:MM A}` (XFA 3.3, "Time Pictures", pp. 1173–1174), or `None` when it isn't one or has
/// a symbol this doesn't read (`k`, `K`, `FFF`, zones, quoted text).
pub(crate) fn time_letters(picture: &str) -> Option<String> {
    // Of several alternatives, the first: the one a field shows and keeps its value in.
    let first = crate::picture::alternatives(picture).into_iter().next()?;
    let inner = first.trim().strip_prefix("time")?.trim_start().strip_prefix('{')?.strip_suffix('}')?;
    let chars: Vec<char> = inner.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut j = i;
        while j < chars.len() && chars[j] == c {
            j += 1;
        }
        let run = j - i;
        match (c, run) {
            ('H' | 'h' | 'M', 1 | 2) => out.extend(std::iter::repeat_n(c, run)),
            // XFA's seconds are `S`; Acrobat's are `s`.
            ('S', 1 | 2) => out.extend(std::iter::repeat_n('s', run)),
            ('A', 1) => out.push_str("tt"),
            (':' | '.' | ' ' | '-' | ',', _) => out.extend(std::iter::repeat_n(c, run)),
            _ => return None,
        }
        i = j;
    }
    out.chars().any(|c| matches!(c, 'H' | 'h')).then_some(out)
}

/// Whether `f` holds a time of day: a date-time widget whose value is a `<time/>` or whose edit
/// or display picture is a time picture.
pub(crate) fn is_time_field(f: &Field) -> bool {
    let time_picture = |p: &Option<String>| p.as_deref().is_some_and(|p| p.trim_start().starts_with("time"));
    f.ui == Ui::DateTimeEdit && (f.time || time_picture(&f.picture) || time_picture(&f.display_picture))
}

/// The pattern a time field is typed in: its edit picture's, else its display picture's, else
/// the canonical one.
pub(crate) fn time_pattern(f: &Field) -> String {
    [&f.picture, &f.display_picture].into_iter().find_map(|p| p.as_deref().and_then(time_letters)).unwrap_or_else(|| CANONICAL_TIME.to_string())
}

/// The tokens of an Acrobat time pattern: runs of `H`, `h`, `M`, `s` or `t`, or a literal
/// character.
fn time_tokens(pattern: &str) -> Vec<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut j = i + 1;
        if "HhMst".contains(c) {
            while j < chars.len() && chars[j] == c {
                j += 1;
            }
        }
        out.push(chars[i..j].iter().collect());
        i = j;
    }
    out
}

/// A canonical time (`HH:MM:SS`, `HH:MM`, `HH`, their basic forms `HHMMSS` and `HHMM`, with
/// optional `.FFF` milliseconds and a zone, which are dropped) as hours, minutes and seconds.
fn read_canonical(text: &str) -> Option<(u32, u32, u32)> {
    let t = text.trim();
    let t = t.strip_suffix('Z').unwrap_or(t);
    // A zone offset after the time: `+01:00`, `-0500`, `+01`.
    let t = match t.find(['+', '-']) {
        Some(at) => {
            let zone = t[at + 1..].replace(':', "");
            if !matches!(zone.len(), 2 | 4) || !zone.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            &t[..at]
        }
        None => t,
    };
    let t = t.split_once('.').map_or(t, |(time, ms)| if !ms.is_empty() && ms.chars().all(|c| c.is_ascii_digit()) { time } else { "" });
    let parts: Vec<&str> = if t.contains(':') {
        t.split(':').collect()
    } else if matches!(t.len(), 2 | 4 | 6) && t.is_ascii() {
        (0..t.len()).step_by(2).map(|i| &t[i..i + 2]).collect()
    } else {
        return None;
    };
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.len() != 2 || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let n = |i: usize| parts.get(i).map_or(Some(0), |p| p.parse::<u32>().ok());
    let (h, m, s) = (n(0)?, n(1)?, n(2)?);
    (h < 24 && m < 60 && s < 60).then_some((h, m, s))
}

/// The canonical time `canonical` shown in `pattern` (`14:30:00` in `h:MM tt` → `2:30 PM`). A
/// value that isn't a canonical time comes back as it is.
pub(crate) fn canonical_to_pattern(canonical: &str, pattern: &str) -> String {
    let Some((h, m, s)) = read_canonical(canonical) else { return canonical.to_string() };
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    let mut out = String::new();
    for t in time_tokens(pattern) {
        match t.as_str() {
            "HH" => out.push_str(&format!("{h:02}")),
            "H" => out.push_str(&h.to_string()),
            "hh" => out.push_str(&format!("{h12:02}")),
            "h" => out.push_str(&h12.to_string()),
            "MM" => out.push_str(&format!("{m:02}")),
            "M" => out.push_str(&m.to_string()),
            "ss" => out.push_str(&format!("{s:02}")),
            "s" => out.push_str(&s.to_string()),
            "tt" => out.push_str(if h < 12 { "AM" } else { "PM" }),
            "t" => out.push_str(if h < 12 { "A" } else { "P" }),
            lit => out.push_str(lit),
        }
    }
    out
}

/// A time typed in `pattern` as the canonical `HH:MM:SS`, or `None` when it doesn't match. The
/// hour letters say whether it is a 12-hour (`h`, with `tt`) or a 24-hour (`H`) hour.
pub(crate) fn pattern_to_canonical(text: &str, pattern: &str) -> Option<String> {
    let mut rest = text.trim();
    let (mut h, mut m, mut s) = (None, 0, 0);
    let mut twelve = false;
    let mut pm = None;
    for t in time_tokens(pattern) {
        match t.as_str() {
            "HH" | "H" | "hh" | "h" | "MM" | "M" | "ss" | "s" => {
                let width = if t.len() == 2 { 2 } else { rest.chars().take_while(char::is_ascii_digit).count().clamp(1, 2) };
                let digits = rest.get(..width).filter(|d| d.chars().all(|c| c.is_ascii_digit()))?;
                let v: u32 = digits.parse().ok()?;
                rest = &rest[width..];
                match t.as_bytes()[0] {
                    b'H' => h = Some(v),
                    b'h' => {
                        twelve = true;
                        h = Some(v);
                    }
                    b'M' => m = v,
                    _ => s = v,
                }
            }
            "tt" | "t" => {
                let upper = rest.to_ascii_uppercase();
                let (is_pm, len) = if upper.starts_with("AM") {
                    (false, 2)
                } else if upper.starts_with("PM") {
                    (true, 2)
                } else if upper.starts_with('A') {
                    (false, 1)
                } else if upper.starts_with('P') {
                    (true, 1)
                } else {
                    return None;
                };
                pm = Some(is_pm);
                rest = &rest[len..];
            }
            // A space in the pattern matches any run of spaces, or none before AM/PM.
            " " => rest = rest.trim_start(),
            lit => rest = rest.strip_prefix(lit)?,
        }
    }
    if !rest.is_empty() {
        return None;
    }
    let mut h = h?;
    if twelve {
        if !(1..=12).contains(&h) {
            return None;
        }
        h %= 12;
        if pm == Some(true) {
            h += 12;
        }
    }
    (h < 24 && m < 60 && s < 60).then(|| format!("{h:02}:{m:02}:{s:02}"))
}

/// What a time field holds, shown in `pattern`: a canonical time, else the text as it is when it
/// already reads in `pattern` or isn't a time at all (XFA keeps input no picture reads, p. 156).
pub(crate) fn time_in_pattern(text: &str, pattern: &str) -> String {
    if pattern_to_canonical(text, pattern).is_some() {
        return text.to_string();
    }
    canonical_to_pattern(text, pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_pictures_become_acrobat_time_patterns() {
        assert_eq!(time_letters("time{h:MM A}").as_deref(), Some("h:MM tt"));
        assert_eq!(time_letters("time{HH:MM:SS}").as_deref(), Some("HH:MM:ss"));
        assert_eq!(time_letters("time{hh.MM}").as_deref(), Some("hh.MM"));
        assert_eq!(time_letters("time{HH:MM}|time{HHMM}").as_deref(), Some("HH:MM"), "the first alternative");
        // Symbols this doesn't read, and pictures that aren't time pictures.
        for p in ["time{HH:MM:SS.FFF}", "time{k:MM}", "time{HH:MM Z}", "date{YYYY-MM-DD}", "time{MM:SS}", "HH:MM", "date{YYYYMMDD}|time{HH:MM}"] {
            assert_eq!(time_letters(p), None, "{p}");
        }
    }

    #[test]
    fn times_go_between_the_canonical_form_and_a_pattern() {
        for (canonical, pattern, shown) in [
            ("14:30:00", "h:MM tt", "2:30 PM"),
            ("00:05:09", "h:MM:ss tt", "12:05:09 AM"),
            ("12:00:00", "hh:MM tt", "12:00 PM"),
            ("09:07:00", "HH:MM", "09:07"),
            ("23:59:59", "H.MM.ss", "23.59.59"),
        ] {
            assert_eq!(canonical_to_pattern(canonical, pattern), shown, "{canonical} in {pattern}");
            assert_eq!(pattern_to_canonical(shown, pattern).as_deref(), Some(canonical), "{shown} in {pattern}");
        }
        // Other canonical spellings read too: basic form, milliseconds, zones.
        for canonical in ["143000", "14:30", "14:30:00.250", "14:30:00Z", "14:30:00+01:00", "14:30:00-0500", "1430"] {
            assert_eq!(canonical_to_pattern(canonical, "h:MM tt"), "2:30 PM", "{canonical}");
        }
        // Typing is forgiving about case and the space before AM/PM.
        assert_eq!(pattern_to_canonical("2:30pm", "h:MM tt").as_deref(), Some("14:30:00"));
        assert_eq!(pattern_to_canonical("2:30 p", "h:MM tt").as_deref(), Some("14:30:00"));
        // Not times: left as they are, and not read.
        for text in ["", "soon", "25:00:00", "14:60", "2026-03-05"] {
            assert_eq!(canonical_to_pattern(text, "h:MM tt"), text, "{text}");
        }
        for (text, pattern) in [("13:30 PM", "h:MM tt"), ("2:30", "h:MM tt"), ("24:00", "HH:MM"), ("2:30 PM extra", "h:MM tt")] {
            assert_eq!(pattern_to_canonical(text, pattern), None, "{text} in {pattern}");
        }
    }
}
