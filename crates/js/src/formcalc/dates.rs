// ── built-in functions: dates, times, and XFA picture clauses ──────────────────────────────

use super::MAX_STRING;
use super::value::{Val, fixed};

/// `NaiveDate::num_days_from_ce` of 1900-01-01 minus one: a civil date's day count rebased so
/// that 1900-01-01 is FormCalc day 1 (and 1970-01-01 is day 25_568).
const FORMCALC_EPOCH_FROM_CE: i64 = 693_595;

/// Days in one 400-year Gregorian cycle; the calendar repeats after it.
const DAYS_PER_400_YEARS: i64 = 146_097;

/// Milliseconds in one 24-hour day.
const MS_PER_DAY: i64 = 86_400_000;

/// Days from 1900-01-01 (day 1, as FormCalc counts) to a civil date.
///
/// chrono's `NaiveDate` stops near ±262143, but FormCalc accepts day numbers up to `MAX_DAYS`
/// (years past 270000). The calendar repeats every 400 years, so chrono converts the date
/// within its cycle and whole cycles are added back; the result matches the proleptic Gregorian
/// count for every year. Callers pass dates `parse_date`/`civil_from_days` already validated;
/// anything else reads as 0.
pub(super) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    use chrono::Datelike;
    let (cycles, year) = (i128::from(y.div_euclid(400)), y.rem_euclid(400));
    i32::try_from(year)
        .ok()
        .and_then(|year| chrono::NaiveDate::from_ymd_opt(year, m, d))
        .and_then(|date| {
            let days = i128::from(date.num_days_from_ce()) - i128::from(FORMCALC_EPOCH_FROM_CE) + cycles * i128::from(DAYS_PER_400_YEARS);
            i64::try_from(days).ok()
        })
        .unwrap_or(0)
}

/// The civil date (year, month, day) at FormCalc day number `n` (day 1 is 1900-01-01), for any
/// `n`: chrono converts the day within its 400-year cycle and the cycles are added back as years.
pub(super) fn civil_from_days(n: i64) -> (i64, u32, u32) {
    use chrono::Datelike;
    // Days since CE day 0 (0000-12-31), in i128 so no `n` overflows.
    let from_ce = i128::from(n) + i128::from(FORMCALC_EPOCH_FROM_CE);
    let (cycles, rest) = (from_ce.div_euclid(i128::from(DAYS_PER_400_YEARS)), from_ce.rem_euclid(i128::from(DAYS_PER_400_YEARS)));
    // `rest` is 0..146_097, so CE day `rest` lies in years 0..=400 (day 0 is 0000-12-31).
    let date = i32::try_from(rest).ok().and_then(chrono::NaiveDate::from_num_days_from_ce_opt);
    match date {
        Some(date) => {
            let y = i128::from(date.year()) + cycles * 400;
            (i64::try_from(y).unwrap_or(if y < 0 { i64::MIN } else { i64::MAX }), date.month(), date.day())
        }
        // Unreachable: every `rest` in 0..146_097 is a valid CE day.
        None => (1900, 1, 1),
    }
}

/// Day numbers (and years) beyond this are refused rather than computed.
pub(super) const MAX_DAYS: f64 = 1e8;

/// The century split of a two-digit year (`YY`), XFA picture clauses' default: `00` to `29` are
/// 2000 to 2029 and `30` to `99` are 1930 to 1999 (XFA 3.3, chapter 26, Picture Clause Reference,
/// p. 1171, which makes 30 the default and lets the century split be set otherwise).
const CENTURY_SPLIT: i64 = 30;

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const MONTHS_SHORT: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const WEEKDAYS_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Characters of the longest month or weekday name: all a name token needs to read of the text.
const LONGEST_NAME: usize = 9;

/// Number of days in month `m` (1-12) of year `y`, leap years included.
pub(crate) fn days_in(y: i64, m: u32) -> u32 {
    let y = y.rem_euclid(400);
    let first = |y: i64, m: u32| i32::try_from(y).ok().and_then(|y| chrono::NaiveDate::from_ymd_opt(y, m, 1));
    let next = if m == 12 { first(y + 1, 1) } else { first(y, m + 1) };
    match (first(y, m), next) {
        (Some(a), Some(b)) => (b - a).num_days() as u32,
        _ => 31,
    }
}

/// Ordinal day of year (`1..=366`) for civil date `(y, m, d)`.
fn day_of_year(y: i64, m: u32, d: u32) -> u32 {
    (1..m.min(13)).map(|mm| days_in(y, mm)).sum::<u32>() + d
}

/// Convert ordinal day of year (`1..=366`) in year `y` to `(month, day)`.
fn month_day_from_doy(y: i64, mut doy: u32) -> Option<(u32, u32)> {
    if doy == 0 {
        return None;
    }
    for m in 1..=12 {
        let dim = days_in(y, m);
        if doy <= dim {
            return Some((m, doy));
        }
        doy -= dim;
    }
    None
}

// ── picture clause categories, wrappers, and alternatives ───────────────────────────────────

/// Category of an XFA picture clause alternative (XFA 3.3 Ch. 26, p. 1146).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PicCat {
    Date,
    Time,
    DateTime,
    Text,
    Num,
    Zero,
    Null,
}

/// A single parsed picture clause alternative with its category, optional locale, and inner pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PicClause {
    pub(super) cat: PicCat,
    pub(super) locale: Option<String>,
    pub(super) pattern: String,
    pub(super) explicit: bool,
}

/// Split a compound picture clause on top-level `|` outside `{...}` braces and `'...'` literals.
pub(super) fn split_alternatives(pic: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut in_quote = false;
    let mut chars = pic.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if in_quote {
            if ch == '\'' && !consume_doubled_quote(&mut chars) {
                in_quote = false;
            }
            continue;
        }
        match ch {
            '\\' => {
                chars.next();
            }
            '\'' => in_quote = true,
            '{' => depth = depth.saturating_add(1),
            '}' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => {
                out.push(pic.get(start..idx).unwrap_or(""));
                start = idx + 1;
            }
            _ => {}
        }
    }
    out.push(pic.get(start..).unwrap_or(""));
    out
}

fn consume_doubled_quote(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> bool {
    if chars.peek().is_some_and(|&(_, next)| next == '\'') {
        chars.next();
        true
    } else {
        false
    }
}

/// Parse all `|`-separated alternatives of a picture string into [`PicClause`] items.
pub(super) fn parse_picture_clauses(pic: &str) -> Vec<PicClause> {
    split_alternatives(pic).into_iter().map(parse_single_clause).collect()
}

fn parse_single_clause(raw: &str) -> PicClause {
    if let Some((cat, locale, inner)) = unwrap_category_wrapper(raw) {
        return PicClause { cat, locale, pattern: inner.to_string(), explicit: true };
    }
    PicClause { cat: infer_category(raw), locale: None, pattern: raw.to_string(), explicit: false }
}

fn parse_cat_keyword(name: &str) -> Option<PicCat> {
    match name.trim().to_ascii_lowercase().as_str() {
        "date" => Some(PicCat::Date),
        "time" => Some(PicCat::Time),
        "datetime" => Some(PicCat::DateTime),
        "text" => Some(PicCat::Text),
        "num" => Some(PicCat::Num),
        "zero" => Some(PicCat::Zero),
        "null" => Some(PicCat::Null),
        _ => None,
    }
}

/// If `raw` is wrapped in `cat{...}` or `cat(locale){...}`, return `(cat, locale, inner)`.
fn unwrap_category_wrapper(raw: &str) -> Option<(PicCat, Option<String>, &str)> {
    let trimmed = raw.trim();
    if !trimmed.ends_with('}') {
        return None;
    }
    let open = find_wrapper_open_brace(trimmed)?;
    let head = trimmed.get(..open)?.trim();
    let (cat_str, locale) = if let Some((c, rest)) = head.split_once('(') {
        let loc = rest.strip_suffix(')')?.trim();
        (c, Some(loc.to_string()))
    } else {
        (head, None)
    };
    let cat = parse_cat_keyword(cat_str)?;
    let inner = trimmed.get(open + 1..trimmed.len() - 1)?;
    Some((cat, locale, inner))
}

fn find_wrapper_open_brace(trimmed: &str) -> Option<usize> {
    let mut open = None;
    let mut depth = 0usize;
    let mut in_quote = false;
    let mut chars = trimmed.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if in_quote {
            if ch == '\'' && !consume_doubled_quote(&mut chars) {
                in_quote = false;
            }
            continue;
        }
        match ch {
            '\\' => {
                if !chars.peek().is_some_and(|&(n_idx, n_ch)| n_ch == '}' && depth == 1 && n_idx + 1 == trimmed.len()) {
                    chars.next();
                }
            }
            '\'' => in_quote = true,
            '{' => {
                if depth == 0 && open.is_none() {
                    open = Some(idx);
                }
                depth = depth.saturating_add(1);
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && idx + 1 != trimmed.len() {
                    return None;
                }
            }
            _ => {}
        }
    }
    if depth == 0 && !in_quote { open } else { None }
}

/// Strip an optional category wrapper from a single pattern string.
fn strip_wrapper(picture: &str) -> &str {
    unwrap_category_wrapper(picture).map_or(picture, |(_, _, inner)| inner)
}

/// Infer the [`PicCat`] of a bare (unwrapped) pattern from its unquoted symbols.
fn infer_category(pattern: &str) -> PicCat {
    let unquoted = collect_unquoted_chars(pattern);
    let has_num_punct = unquoted.iter().any(|&c| matches!(c, 'z' | 'Z' | '.' | ',' | '$' | '%'));
    let has_date = has_date_symbols(&unquoted, has_num_punct);
    let has_time = has_time_symbols(&unquoted, has_num_punct);
    if has_date && has_time {
        return PicCat::DateTime;
    }
    if has_time {
        return PicCat::Time;
    }
    if has_date {
        return PicCat::Date;
    }
    if is_bare_text_pattern(&unquoted, has_num_punct) {
        return PicCat::Text;
    }
    PicCat::Num
}

fn collect_unquoted_chars(pattern: &str) -> Vec<char> {
    let pic: Vec<char> = pattern.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&ch) = pic.get(i) {
        if ch == '\'' {
            i = skip_quoted_literal(&pic, i + 1, &mut None);
        } else if ch == '\\' {
            i += 2;
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

fn has_date_symbols(chars: &[char], has_num_punct: bool) -> bool {
    let mut i = 0;
    while let Some(&ch) = chars.get(i) {
        let up = ch.to_ascii_uppercase();
        if up == 'D' && chars.get(i + 1).is_some_and(|c| c.eq_ignore_ascii_case(&'B')) && has_num_punct {
            i += 2;
            continue;
        }
        if matches!(up, 'Y' | 'D' | 'E' | 'J') {
            return true;
        }
        if up == 'M'
            && chars.get(i + 1).is_some_and(|c| c.eq_ignore_ascii_case(&'M'))
            && chars.get(i + 2).is_some_and(|c| c.eq_ignore_ascii_case(&'M'))
        {
            return true;
        }
        i += 1;
    }
    false
}

fn has_time_symbols(chars: &[char], has_num_punct: bool) -> bool {
    let has_colon = chars.contains(&':');
    for &ch in chars {
        if matches!(ch, 'H' | 'h' | 'k' | 'K' | 'F') {
            return true;
        }
        if ch == 'S' && (has_colon || !has_num_punct && !chars.contains(&'9')) {
            return true;
        }
        if ch == 'M' && has_colon {
            return true;
        }
    }
    false
}

fn is_bare_text_pattern(chars: &[char], has_num_punct: bool) -> bool {
    let mut i = 0;
    let mut has_text_slot = false;
    let mut nine_runs = 0usize;
    let mut in_nine_run = false;
    while let Some(&ch) = chars.get(i) {
        let up = ch.to_ascii_uppercase();
        if (up == 'C' && chars.get(i + 1).is_some_and(|c| c.eq_ignore_ascii_case(&'R')))
            || (up == 'D' && chars.get(i + 1).is_some_and(|c| c.eq_ignore_ascii_case(&'B')))
        {
            in_nine_run = false;
            i += 2;
            continue;
        }
        if matches!(ch, 'A' | 'X' | 'O' | '0') {
            has_text_slot = true;
        }
        if ch == '9' {
            if !in_nine_run {
                nine_runs += 1;
                in_nine_run = true;
            }
        } else {
            in_nine_run = false;
        }
        i += 1;
    }
    if has_text_slot {
        return true;
    }
    !has_num_punct && nine_runs >= 2
}

/// Advance past a `'...'` quoted literal starting after the opening `'`, appending unescaped chars if `out` is `Some`.
fn skip_quoted_literal(pic: &[char], mut i: usize, out: &mut Option<&mut String>) -> usize {
    while let Some(&ch) = pic.get(i) {
        if ch == '\'' {
            if pic.get(i + 1) == Some(&'\'') {
                if let Some(buf) = out.as_deref_mut() {
                    buf.push('\'');
                }
                i += 2;
            } else {
                return i + 1;
            }
        } else {
            if let Some(buf) = out.as_deref_mut() {
                buf.push(ch);
            }
            i += 1;
        }
    }
    i
}

/// Match a `'...'` quoted literal in `pic` (starting after the opening `'`) against `txt` at `*j`.
fn match_quoted_literal(pic: &[char], mut i: usize, txt: &[char], j: &mut usize) -> Option<usize> {
    while let Some(&ch) = pic.get(i) {
        if ch == '\'' {
            if pic.get(i + 1) == Some(&'\'') {
                if txt.get(*j) != Some(&'\'') {
                    return None;
                }
                *j += 1;
                i += 2;
            } else {
                return Some(i + 1);
            }
        } else {
            if txt.get(*j) != Some(&ch) {
                return None;
            }
            *j += 1;
            i += 1;
        }
    }
    Some(i)
}

/// Render a literal clause (`null{...}` or literal `zero{...}`), unescaping `'...'` (`''` -> `'`) and `\c`.
pub(super) fn render_literal_clause(pattern: &str) -> String {
    let pic: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while let Some(&ch) = pic.get(i) {
        if out.len() > MAX_STRING {
            break;
        }
        if ch == '\'' {
            i = skip_quoted_literal(&pic, i + 1, &mut Some(&mut out));
        } else if ch == '\\' {
            if let Some(&next) = pic.get(i + 1) {
                out.push(next);
                i += 2;
            } else {
                out.push('\\');
                i += 1;
            }
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

/// Render a `zero{...}` picture clause for numeric `0.0`.
pub(super) fn format_zero_pic(pattern: &str) -> String {
    let unquoted = collect_unquoted_chars(pattern);
    if unquoted.iter().any(|&c| matches!(c, '9' | 'z' | 'Z')) { format_num_pic(0.0, pattern) } else { render_literal_clause(pattern) }
}

// ── date parsing and formatting ─────────────────────────────────────────────────────────────

#[derive(Default)]
struct DateParts {
    y: Option<i64>,
    m: Option<u32>,
    d: Option<u32>,
    doy: Option<u32>,
}

/// Parse `text` as a date by `picture` (`YYYY-MM-DD`, `MM/DD/YYYY`, `MMM D, YYYY`, `date{...}`, …).
pub(super) fn parse_date(text: &str, picture: &str) -> Option<(i64, u32, u32)> {
    parse_date_mode(text, picture, false)
}

/// Parse `text` as a date by `picture`, requiring the entire trimmed input to be consumed.
pub(super) fn parse_date_exact(text: &str, picture: &str) -> Option<(i64, u32, u32)> {
    parse_date_mode(text, picture, true)
}

fn parse_date_mode(text: &str, picture: &str, exact: bool) -> Option<(i64, u32, u32)> {
    for alt in split_alternatives(picture) {
        let pat = strip_wrapper(alt);
        if let Some(res) = parse_date_single(text, pat, exact) {
            return Some(res);
        }
    }
    None
}

fn parse_date_single(text: &str, picture: &str, exact: bool) -> Option<(i64, u32, u32)> {
    let pic: Vec<char> = picture.chars().collect();
    let txt: Vec<char> = text.trim().chars().collect();
    let mut parts = DateParts::default();
    let (mut i, mut j) = (0, 0);
    while let Some(&ch) = pic.get(i) {
        if ch == '\'' {
            i = match_quoted_literal(&pic, i + 1, &txt, &mut j)?;
            continue;
        }
        if ch == '\\' {
            let expected = *pic.get(i + 1).unwrap_or(&'\\');
            if txt.get(j) != Some(&expected) {
                return None;
            }
            i += (pic.get(i + 1).is_some() as usize) + 1;
            j += 1;
            continue;
        }
        let c = ch.to_ascii_uppercase();
        if matches!(c, 'Y' | 'M' | 'D' | 'E' | 'J') {
            let run = count_ascii_case_run(&pic, i, c);
            i += run;
            parse_date_token(c, run, &txt, &mut j, &mut parts)?;
        } else {
            if txt.get(j) != Some(&ch) {
                return None;
            }
            i += 1;
            j += 1;
        }
    }
    if exact && j != txt.len() {
        return None;
    }
    finalize_date_parts(parts)
}

fn count_ascii_case_run(pic: &[char], start: usize, upper: char) -> usize {
    let mut run = 1;
    while pic.get(start + run).is_some_and(|c| c.to_ascii_uppercase() == upper) {
        run += 1;
    }
    run
}

fn count_exact_run(pic: &[char], start: usize, target: char) -> usize {
    let mut run = 1;
    while pic.get(start + run) == Some(&target) {
        run += 1;
    }
    run
}

fn parse_date_token(c: char, run: usize, txt: &[char], j: &mut usize, parts: &mut DateParts) -> Option<()> {
    match c {
        'M' if run >= 3 => {
            let (idx, len) = match_name_prefix(txt, *j, &MONTHS, &MONTHS_SHORT)?;
            parts.m = Some(idx as u32 + 1);
            *j += len;
        }
        'E' => {
            if let Some((_, len)) = match_name_prefix(txt, *j, &WEEKDAYS, &WEEKDAYS_SHORT) {
                *j += len;
            } else {
                let v = read_uint_digits(txt, j, 1)?;
                if !(1..=7).contains(&v) {
                    return None;
                }
            }
        }
        'J' => {
            let width = if run <= 3 { 3 } else { run };
            parts.doy = Some(read_uint_digits(txt, j, width)? as u32);
        }
        _ => {
            let width = if run == 1 { 2 } else { run };
            let v = read_uint_digits(txt, j, width)?;
            match c {
                'Y' => parts.y = Some(if run <= 2 { if v < CENTURY_SPLIT { 2000 + v } else { 1900 + v } } else { v }),
                'M' => parts.m = Some(v as u32),
                _ => parts.d = Some(v as u32),
            }
        }
    }
    Some(())
}

fn match_name_prefix(txt: &[char], j: usize, full_names: &[&str], short_names: &[&str]) -> Option<(usize, usize)> {
    let rest: String = txt.get(j..).unwrap_or(&[]).iter().take(LONGEST_NAME).collect();
    let lower = rest.to_ascii_lowercase();
    for (idx, (&full, &short)) in full_names.iter().zip(short_names.iter()).enumerate() {
        let full_lower = full.to_ascii_lowercase();
        if lower.starts_with(&full_lower) {
            return Some((idx, full.chars().count()));
        }
        let short_lower = short.to_ascii_lowercase();
        if lower.starts_with(&short_lower) {
            return Some((idx, short.chars().count()));
        }
    }
    None
}

fn read_uint_digits(txt: &[char], j: &mut usize, max_width: usize) -> Option<i64> {
    let mut digits = String::new();
    while digits.len() < max_width && txt.get(*j).is_some_and(char::is_ascii_digit) {
        if let Some(&ch) = txt.get(*j) {
            digits.push(ch);
        }
        *j += 1;
    }
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

fn finalize_date_parts(parts: DateParts) -> Option<(i64, u32, u32)> {
    let y = parts.y?;
    if !(-200_000..=200_000).contains(&y) {
        return None;
    }
    let (m, d) = if let (None, None, Some(doy)) = (parts.m, parts.d, parts.doy) {
        month_day_from_doy(y, doy)?
    } else {
        (parts.m.unwrap_or(1), parts.d.unwrap_or(1))
    };
    if let Some(doy) = parts.doy
        && day_of_year(y, m, d) != doy
    {
        return None;
    }
    ((1..=12).contains(&m) && (1..=days_in(y, m)).contains(&d)).then_some((y, m, d))
}

/// Format civil date `(y, m, d)` according to `picture`.
pub(super) fn format_date(y: i64, m: u32, d: u32, picture: &str) -> String {
    let first_alt = split_alternatives(picture).into_iter().next().unwrap_or(picture);
    let pat = strip_wrapper(first_alt);
    let pic: Vec<char> = pat.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while let Some(&ch) = pic.get(i) {
        if out.len() > MAX_STRING {
            break;
        }
        if ch == '\'' {
            i = skip_quoted_literal(&pic, i + 1, &mut Some(&mut out));
            continue;
        }
        if ch == '\\' {
            if let Some(&next) = pic.get(i + 1) {
                out.push(next);
                i += 2;
            } else {
                out.push('\\');
                i += 1;
            }
            continue;
        }
        let c = ch.to_ascii_uppercase();
        if matches!(c, 'Y' | 'M' | 'D' | 'E' | 'J') {
            let run = count_ascii_case_run(&pic, i, c);
            i += run;
            append_date_token(&mut out, y, m, d, c, run);
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

fn append_date_token(out: &mut String, y: i64, m: u32, d: u32, c: char, run: usize) {
    match (c, run) {
        ('Y', 1 | 2) => out.push_str(&format!("{:02}", y.rem_euclid(100))),
        ('Y', _) => out.push_str(&format!("{y:04}")),
        ('M', 1) => out.push_str(&m.to_string()),
        ('M', 2) => out.push_str(&format!("{m:02}")),
        ('M', 3) => out.push_str(MONTHS_SHORT.get((m as usize).saturating_sub(1)).copied().unwrap_or("")),
        ('M', _) => out.push_str(MONTHS.get((m as usize).saturating_sub(1)).copied().unwrap_or("")),
        ('D', 1) => out.push_str(&d.to_string()),
        ('D', _) => out.push_str(&format!("{d:02}")),
        ('E', 1 | 2) => {
            let weekday = days_from_civil(y, m, d).rem_euclid(7) as usize;
            out.push_str(&(weekday + 1).to_string());
        }
        ('E', 3) => {
            let weekday = days_from_civil(y, m, d).rem_euclid(7) as usize;
            out.push_str(WEEKDAYS_SHORT.get(weekday).copied().unwrap_or(""));
        }
        ('E', _) => {
            let weekday = days_from_civil(y, m, d).rem_euclid(7) as usize;
            out.push_str(WEEKDAYS.get(weekday).copied().unwrap_or(""));
        }
        ('J', 1) => out.push_str(&day_of_year(y, m, d).to_string()),
        ('J', 2) => out.push_str(&format!("{:02}", day_of_year(y, m, d))),
        (_, _) => out.push_str(&format!("{:03}", day_of_year(y, m, d))),
    }
}

// ── time and datetime parsing and formatting (XFA 3.3 pp. 1154–1158) ────────────────────────

#[derive(Clone, Copy)]
enum HourToken {
    H24(u32), // H / HH: 0..=23
    K24(u32), // k / kk: 1..=24 (24 = 0)
    H12(u32), // h / hh: 1..=12 (12 = 0 AM / 12 PM)
    K12(u32), // K / KK: 0..=11 (0 = 0 AM / 12 PM)
}

#[derive(Default)]
struct TimeParts {
    hour: Option<HourToken>,
    min: u32,
    sec: u32,
    millis: u32,
    pm: Option<bool>,
    tz_offset_ms: Option<i64>,
    seen_any: bool,
}

impl TimeParts {
    fn into_hms_millis(self) -> Option<(u32, u32, u32, u32)> {
        self.into_hms_millis_with_day_delta().map(|(h, m, s, ms, _)| (h, m, s, ms))
    }

    fn into_hms_millis_with_day_delta(self) -> Option<(u32, u32, u32, u32, i64)> {
        if !self.seen_any || self.min > 59 || self.sec > 59 || self.millis > 999 {
            return None;
        }
        let h = match (self.hour.unwrap_or(HourToken::H24(0)), self.pm) {
            (HourToken::H24(v), _) if v <= 23 => v,
            (HourToken::K24(v), _) if (1..=24).contains(&v) => {
                if v == 24 {
                    0
                } else {
                    v
                }
            }
            (HourToken::H12(v), Some(false) | None) if (1..=12).contains(&v) => v % 12,
            (HourToken::H12(v), Some(true)) if (1..=12).contains(&v) => (v % 12) + 12,
            (HourToken::K12(v), Some(false) | None) if v <= 11 => v,
            (HourToken::K12(v), Some(true)) if v <= 11 => v + 12,
            _ => return None,
        };
        let local_ms = (i64::from(h) * 3600 + i64::from(self.min) * 60 + i64::from(self.sec)) * 1000 + i64::from(self.millis);
        let shifted = local_ms - self.tz_offset_ms.unwrap_or(0);
        let day_delta = shifted.div_euclid(MS_PER_DAY);
        let gmt_ms = shifted.rem_euclid(MS_PER_DAY) as u32;
        let (hh, mm, ss, ms) = ms_to_hms_millis(gmt_ms);
        Some((hh, mm, ss, ms, day_delta))
    }
}

fn ms_to_hms_millis(total_ms: u32) -> (u32, u32, u32, u32) {
    let h = (total_ms / 3_600_000) % 24;
    let m = (total_ms / 60_000) % 60;
    let s = (total_ms / 1_000) % 60;
    let millis = total_ms % 1_000;
    (h, m, s, millis)
}

fn hms_millis_to_ms(h: u32, m: u32, s: u32, millis: u32) -> u32 {
    (h * 3600 + m * 60 + s) * 1000 + millis
}

/// Format milliseconds since midnight according to an XFA time picture (`time{...}` or bare).
pub(super) fn format_time(ms_f64: f64, picture: &str) -> String {
    let first_alt = split_alternatives(picture).into_iter().next().unwrap_or(picture);
    let pat = strip_wrapper(first_alt);
    let total_ms = if ms_f64.is_finite() { (ms_f64.round().rem_euclid(MS_PER_DAY as f64) as i64).rem_euclid(MS_PER_DAY) as u32 } else { 0 };
    let (h, m, s, millis) = ms_to_hms_millis(total_ms);
    let pic: Vec<char> = pat.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while let Some(&ch) = pic.get(i) {
        if out.len() > MAX_STRING {
            break;
        }
        if ch == '\'' {
            i = skip_quoted_literal(&pic, i + 1, &mut Some(&mut out));
            continue;
        }
        if ch == '\\' {
            if let Some(&next) = pic.get(i + 1) {
                out.push(next);
                i += 2;
            } else {
                out.push('\\');
                i += 1;
            }
            continue;
        }
        if matches!(ch, 'H' | 'h' | 'k' | 'K' | 'M' | 'S' | 'F' | 'A' | 'Z' | 'z') {
            let run = count_exact_run(&pic, i, ch);
            i += run;
            append_time_token(&mut out, h, m, s, millis, ch, run);
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

fn append_time_token(out: &mut String, h: u32, m: u32, s: u32, millis: u32, ch: char, run: usize) {
    match ch {
        'H' => {
            if run == 1 {
                out.push_str(&h.to_string());
            } else {
                out.push_str(&format!("{h:02}"));
            }
        }
        'k' => {
            let kh = if h == 0 { 24 } else { h };
            if run == 1 {
                out.push_str(&kh.to_string());
            } else {
                out.push_str(&format!("{kh:02}"));
            }
        }
        'h' => {
            let h12 = match h % 12 {
                0 => 12,
                x => x,
            };
            if run == 1 {
                out.push_str(&h12.to_string());
            } else {
                out.push_str(&format!("{h12:02}"));
            }
        }
        'K' => {
            let k12 = h % 12;
            if run == 1 {
                out.push_str(&k12.to_string());
            } else {
                out.push_str(&format!("{k12:02}"));
            }
        }
        'M' => {
            if run == 1 {
                out.push_str(&m.to_string());
            } else {
                out.push_str(&format!("{m:02}"));
            }
        }
        'S' => {
            if run == 1 {
                out.push_str(&s.to_string());
            } else {
                out.push_str(&format!("{s:02}"));
            }
        }
        'F' => match run {
            1 => out.push_str(&(millis / 100).to_string()),
            2 => out.push_str(&format!("{:02}", millis / 10)),
            _ => out.push_str(&format!("{millis:03}")),
        },
        'A' => {
            let pm = h >= 12;
            if run == 1 {
                out.push_str(if pm { "P" } else { "A" });
            } else {
                out.push_str(if pm { "PM" } else { "AM" });
            }
        }
        'z' => out.push_str("GMT"),
        _ => out.push('Z'),
    }
}

/// Format `(h, m, s, millis)` into canonical XFA ISO time (`HH:MM:SS` or `HH:MM:SS.FFF`).
pub(super) fn format_time_iso(h: u32, m: u32, s: u32, millis: u32) -> String {
    if millis == 0 { format!("{h:02}:{m:02}:{s:02}") } else { format!("{h:02}:{m:02}:{s:02}.{millis:03}") }
}

/// Parse `text` against a time `picture`, returning `(h, m, s, millis)` in GMT.
pub(super) fn parse_time_parts(text: &str, picture: &str, exact: bool) -> Option<(u32, u32, u32, u32)> {
    for alt in split_alternatives(picture) {
        let pat = strip_wrapper(alt);
        if let Some(res) = parse_time_single(text, pat, exact) {
            return Some(res);
        }
    }
    None
}

/// Parse `text` against a time `picture`, returning milliseconds since midnight.
pub(super) fn parse_time_ms(text: &str, picture: &str) -> Option<u32> {
    parse_time_parts(text, picture, false).map(|(h, m, s, ms)| hms_millis_to_ms(h, m, s, ms))
}

fn parse_time_single(text: &str, picture: &str, exact: bool) -> Option<(u32, u32, u32, u32)> {
    let pic: Vec<char> = picture.chars().collect();
    let txt: Vec<char> = text.trim().chars().collect();
    let mut parts = TimeParts::default();
    let (mut i, mut j) = (0, 0);
    while let Some(&ch) = pic.get(i) {
        if ch == '\'' {
            i = match_quoted_literal(&pic, i + 1, &txt, &mut j)?;
            continue;
        }
        if ch == '\\' {
            let expected = *pic.get(i + 1).unwrap_or(&'\\');
            if txt.get(j) != Some(&expected) {
                return None;
            }
            i += (pic.get(i + 1).is_some() as usize) + 1;
            j += 1;
            continue;
        }
        if matches!(ch, 'H' | 'h' | 'k' | 'K' | 'M' | 'S' | 'F' | 'A' | 'Z' | 'z') {
            let run = count_exact_run(&pic, i, ch);
            i += run;
            let next_pic = pic.get(i).copied();
            parse_time_token(ch, run, next_pic, &txt, &mut j, &mut parts)?;
        } else if ch == ' ' && pic.get(i + 1).is_some_and(|c| matches!(c, 'Z' | 'z' | 'A')) {
            if txt.get(j) == Some(&' ') {
                j += 1;
            }
            i += 1;
        } else {
            if txt.get(j) != Some(&ch) {
                return None;
            }
            i += 1;
            j += 1;
        }
    }
    if exact && j != txt.len() {
        return None;
    }
    parts.into_hms_millis()
}

fn parse_time_token(ch: char, run: usize, next_pic: Option<char>, txt: &[char], j: &mut usize, parts: &mut TimeParts) -> Option<()> {
    parts.seen_any = true;
    match ch {
        'H' | 'h' | 'k' | 'K' | 'M' | 'S' => {
            let v = read_uint_digits(txt, j, 2)? as u32;
            match ch {
                'H' => parts.hour = Some(HourToken::H24(v)),
                'k' => parts.hour = Some(HourToken::K24(v)),
                'h' => parts.hour = Some(HourToken::H12(v)),
                'K' => parts.hour = Some(HourToken::K12(v)),
                'M' => parts.min = v,
                _ => parts.sec = v,
            }
        }
        'F' => {
            let max_w = run.min(3);
            let start = *j;
            let v = read_uint_digits(txt, j, max_w)? as u32;
            let len = *j - start;
            parts.millis = match len {
                1 => v * 100,
                2 => v * 10,
                _ => v,
            };
        }
        'A' => {
            let first = *txt.get(*j)?;
            if !matches!(first, 'A' | 'a' | 'P' | 'p') {
                return None;
            }
            parts.pm = Some(matches!(first, 'P' | 'p'));
            *j += 1;
            if run >= 2 {
                if !txt.get(*j).is_some_and(|c| matches!(c, 'M' | 'm')) {
                    return None;
                }
                *j += 1;
            } else if txt.get(*j).is_some_and(|c| matches!(c, 'M' | 'm')) && next_pic != Some('M') {
                *j += 1;
            }
        }
        _ => {
            parts.tz_offset_ms = Some(parse_tz_offset(txt, j)?);
        }
    }
    Some(())
}

fn parse_tz_offset(txt: &[char], j: &mut usize) -> Option<i64> {
    while txt.get(*j) == Some(&' ') {
        *j += 1;
    }
    if txt.get(*j).is_some_and(|c| matches!(c, 'Z' | 'z')) {
        *j += 1;
        return Some(0);
    }
    if matches_ascii_prefix(txt, *j, "GMT") || matches_ascii_prefix(txt, *j, "UTC") {
        *j += 3;
        if !txt.get(*j).is_some_and(|c| matches!(c, '+' | '-')) {
            return Some(0);
        }
    }
    let sign = match txt.get(*j)? {
        '+' => 1i64,
        '-' => -1i64,
        _ => return None,
    };
    *j += 1;
    let tz_h = read_uint_digits(txt, j, 2)?;
    if !(0..=23).contains(&tz_h) {
        return None;
    }
    let tz_m = if txt.get(*j) == Some(&':') {
        *j += 1;
        read_uint_digits(txt, j, 2)?
    } else if txt.get(*j).is_some_and(char::is_ascii_digit) {
        read_uint_digits(txt, j, 2)?
    } else {
        0
    };
    if !(0..=59).contains(&tz_m) {
        return None;
    }
    Some(sign * (tz_h * 60 + tz_m) * 60_000)
}

fn matches_ascii_prefix(txt: &[char], j: usize, prefix: &str) -> bool {
    prefix.chars().enumerate().all(|(k, p)| txt.get(j + k).is_some_and(|c| c.eq_ignore_ascii_case(&p)))
}

/// Parse an ISO or common time string (`14:30:05`, `14:30:05.250`, `T12:30:45Z`, `14:30:00-05:00`, `143005`) into GMT milliseconds.
pub(super) fn parse_iso_time(text: &str) -> Option<u32> {
    parse_iso_time_with_day_delta(text).map(|(ms, _)| ms)
}

fn parse_iso_time_with_day_delta(text: &str) -> Option<(u32, i64)> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let t = t.strip_prefix(['T', 't']).unwrap_or(t);
    for pat in [
        "HH:MM:SS.FFF Z",
        "HH:MM:SS.FFF",
        "HH:MM:SS Z",
        "HH:MM:SS",
        "HH:MM Z",
        "HH:MM",
        "h:MM:SS.FFF A Z",
        "h:MM:SS.FFF A",
        "h:MM:SS A Z",
        "h:MM:SS A",
        "h:MM A Z",
        "h:MM A",
        "HHMMSS.FFF Z",
        "HHMMSS.FFF",
        "HHMMSS Z",
        "HHMMSS",
    ] {
        if let Some((h, m, s, ms, day_delta)) = parse_time_single_with_delta(t, pat, true) {
            return Some((hms_millis_to_ms(h, m, s, ms), day_delta));
        }
    }
    None
}

fn parse_time_single_with_delta(text: &str, picture: &str, exact: bool) -> Option<(u32, u32, u32, u32, i64)> {
    let pic: Vec<char> = picture.chars().collect();
    let txt: Vec<char> = text.trim().chars().collect();
    let mut parts = TimeParts::default();
    let (mut i, mut j) = (0, 0);
    while let Some(&ch) = pic.get(i) {
        if ch == '\'' {
            i = match_quoted_literal(&pic, i + 1, &txt, &mut j)?;
            continue;
        }
        if ch == '\\' {
            let expected = *pic.get(i + 1).unwrap_or(&'\\');
            if txt.get(j) != Some(&expected) {
                return None;
            }
            i += (pic.get(i + 1).is_some() as usize) + 1;
            j += 1;
            continue;
        }
        if matches!(ch, 'H' | 'h' | 'k' | 'K' | 'M' | 'S' | 'F' | 'A' | 'Z' | 'z') {
            let run = count_exact_run(&pic, i, ch);
            i += run;
            let next_pic = pic.get(i).copied();
            parse_time_token(ch, run, next_pic, &txt, &mut j, &mut parts)?;
        } else if ch == ' ' && pic.get(i + 1).is_some_and(|c| matches!(c, 'Z' | 'z' | 'A')) {
            if txt.get(j) == Some(&' ') {
                j += 1;
            }
            i += 1;
        } else {
            if txt.get(j) != Some(&ch) {
                return None;
            }
            i += 1;
            j += 1;
        }
    }
    if exact && j != txt.len() {
        return None;
    }
    parts.into_hms_millis_with_day_delta()
}

// ── datetime parsing and formatting (`datetime{...}`) ───────────────────────────────────────

fn is_datetime_time_token(pic: &[char], i: usize, ch: char, in_time: bool) -> bool {
    if matches!(ch, 'H' | 'h' | 'k' | 'K' | 'S' | 'F' | 'A' | 'Z' | 'z') {
        return true;
    }
    if ch != 'M' {
        return false;
    }
    let run = count_exact_run(pic, i, 'M');
    if run > 2 || pic.get(i + run).is_some_and(|c| matches!(c, '/' | '-')) {
        return false;
    }
    if (i > 0 && pic.get(i - 1) == Some(&':')) || pic.get(i + run) == Some(&':') {
        return true;
    }
    in_time
}

/// Format a date and time value (`2024-02-29T14:30:05`, day number, etc.) with a `datetime` picture.
pub(super) fn format_datetime(text: &str, days_opt: Option<i64>, picture: &str) -> Option<String> {
    let (y, m, d, h, min, s, millis) = if let Some(days) = days_opt {
        let (y, m, d) = civil_from_days(days);
        (y, m, d, 0, 0, 0, 0)
    } else {
        parse_iso_datetime(text)?
    };
    let first_alt = split_alternatives(picture).into_iter().next().unwrap_or(picture);
    let pat = strip_wrapper(first_alt);
    let pic: Vec<char> = pat.chars().collect();
    let mut out = String::new();
    let mut in_time = false;
    let mut i = 0;
    while let Some(&ch) = pic.get(i) {
        if out.len() > MAX_STRING {
            break;
        }
        if ch == '\'' {
            i = skip_quoted_literal(&pic, i + 1, &mut Some(&mut out));
            continue;
        }
        if ch == '\\' {
            if let Some(&next) = pic.get(i + 1) {
                out.push(next);
                i += 2;
            } else {
                out.push('\\');
                i += 1;
            }
            continue;
        }
        let up = ch.to_ascii_uppercase();
        if is_datetime_time_token(&pic, i, ch, in_time) {
            in_time = true;
            let run = count_exact_run(&pic, i, ch);
            i += run;
            append_time_token(&mut out, h, min, s, millis, ch, run);
        } else if matches!(up, 'Y' | 'M' | 'D' | 'E' | 'J') {
            in_time = false;
            let run = count_ascii_case_run(&pic, i, up);
            i += run;
            append_date_token(&mut out, y, m, d, up, run);
        } else {
            out.push(ch);
            i += 1;
        }
    }
    Some(out)
}

fn parse_iso_datetime(text: &str) -> Option<(i64, u32, u32, u32, u32, u32, u32)> {
    let t = text.trim();
    if let Some((d_part, t_part)) = t.split_once(['T', 't', ' ']) {
        let (mut y, mut m, mut d) = parse_date_exact(d_part, "YYYY-MM-DD").or_else(|| parse_date_exact(d_part, "YYYYMMDD"))?;
        let (ms, day_delta) = parse_iso_time_with_day_delta(t_part)?;
        if day_delta != 0 {
            (y, m, d) = civil_from_days(days_from_civil(y, m, d).saturating_add(day_delta));
        }
        let (h, min, s, millis) = ms_to_hms_millis(ms);
        return Some((y, m, d, h, min, s, millis));
    }
    let (y, m, d) = parse_date_exact(t, "YYYY-MM-DD").or_else(|| parse_date_exact(t, "YYYYMMDD"))?;
    Some((y, m, d, 0, 0, 0, 0))
}

/// Parse `text` against a `datetime` picture into canonical ISO datetime `YYYY-MM-DDTHH:MM:SS` (or `.FFF`).
pub(super) fn parse_datetime(text: &str, picture: &str) -> Option<String> {
    let pat = strip_wrapper(picture);
    let pic: Vec<char> = pat.chars().collect();
    let txt: Vec<char> = text.trim().chars().collect();
    let mut d_parts = DateParts::default();
    let mut t_parts = TimeParts::default();
    let mut in_time = false;
    let (mut i, mut j) = (0, 0);
    while let Some(&ch) = pic.get(i) {
        if ch == '\'' {
            i = match_quoted_literal(&pic, i + 1, &txt, &mut j)?;
            continue;
        }
        if ch == '\\' {
            let expected = *pic.get(i + 1).unwrap_or(&'\\');
            if txt.get(j) != Some(&expected) {
                return None;
            }
            i += (pic.get(i + 1).is_some() as usize) + 1;
            j += 1;
            continue;
        }
        let up = ch.to_ascii_uppercase();
        if is_datetime_time_token(&pic, i, ch, in_time) {
            in_time = true;
            let run = count_exact_run(&pic, i, ch);
            i += run;
            let next_pic = pic.get(i).copied();
            parse_time_token(ch, run, next_pic, &txt, &mut j, &mut t_parts)?;
        } else if matches!(up, 'Y' | 'M' | 'D' | 'E' | 'J') {
            in_time = false;
            let run = count_ascii_case_run(&pic, i, up);
            i += run;
            parse_date_token(up, run, &txt, &mut j, &mut d_parts)?;
        } else if ch == ' ' && pic.get(i + 1).is_some_and(|c| matches!(c, 'Z' | 'z' | 'A')) {
            if txt.get(j) == Some(&' ') {
                j += 1;
            }
            i += 1;
        } else {
            if txt.get(j) != Some(&ch) {
                return None;
            }
            i += 1;
            j += 1;
        }
    }
    if j != txt.len() {
        return None;
    }
    let (mut y, mut m, mut d) = finalize_date_parts(d_parts)?;
    let (h, min, s, millis, day_delta) = t_parts.into_hms_millis_with_day_delta()?;
    if day_delta != 0 {
        (y, m, d) = civil_from_days(days_from_civil(y, m, d).saturating_add(day_delta));
    }
    let date_iso = format_date(y, m, d, "YYYY-MM-DD");
    let time_iso = format_time_iso(h, min, s, millis);
    Some(format!("{date_iso}T{time_iso}"))
}

// ── text picture clauses (`text{...}` — XFA 3.3 p. 1152) ────────────────────────────────────

enum TextToken {
    Alpha,
    Any,
    Alnum,
    Digit,
    Sign,
    Lit(char),
}

impl TextToken {
    fn is_slot(&self) -> bool {
        !matches!(self, Self::Lit(_))
    }

    fn accepts(&self, ch: char) -> bool {
        match self {
            Self::Alpha => ch.is_alphabetic(),
            Self::Any => true,
            Self::Alnum => ch.is_alphanumeric(),
            Self::Digit => ch.is_ascii_digit(),
            Self::Sign => matches!(ch, '+' | '-' | ' '),
            Self::Lit(c) => ch == *c,
        }
    }
}

fn tokenize_text_picture(pattern: &str) -> Vec<TextToken> {
    let pat = strip_wrapper(pattern);
    let pic: Vec<char> = pat.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(&ch) = pic.get(i) {
        if ch == '\'' {
            let mut lit = String::new();
            i = skip_quoted_literal(&pic, i + 1, &mut Some(&mut lit));
            tokens.extend(lit.chars().map(TextToken::Lit));
        } else if ch == '\\' {
            if let Some(&next) = pic.get(i + 1) {
                tokens.push(TextToken::Lit(next));
                i += 2;
            } else {
                tokens.push(TextToken::Lit('\\'));
                i += 1;
            }
        } else {
            let tok = match ch {
                'A' => TextToken::Alpha,
                'X' => TextToken::Any,
                'O' | '0' => TextToken::Alnum,
                '9' => TextToken::Digit,
                '+' => TextToken::Sign,
                other => TextToken::Lit(other),
            };
            tokens.push(tok);
            i += 1;
        }
    }
    tokens
}

/// Format `text` according to a text picture clause; returns `None` if `text` does not fit the slots.
pub(super) fn format_text_pic(text: &str, pattern: &str) -> Option<String> {
    let tokens = tokenize_text_picture(pattern);
    let slot_count = tokens.iter().filter(|t| t.is_slot()).count();
    let input: Vec<char> = text.chars().collect();
    if input.len() != slot_count {
        return None;
    }
    let mut out = String::new();
    let mut k = 0;
    for tok in &tokens {
        if out.len() > MAX_STRING {
            break;
        }
        match tok {
            TextToken::Lit(c) => out.push(*c),
            slot => {
                let ch = *input.get(k)?;
                if !slot.accepts(ch) {
                    return None;
                }
                out.push(ch);
                k += 1;
            }
        }
    }
    Some(out)
}

/// Parse `text` against a text picture clause, extracting only the placeholder characters.
pub(super) fn parse_text_pic(text: &str, pattern: &str) -> Option<String> {
    let tokens = tokenize_text_picture(pattern);
    let input: Vec<char> = text.chars().collect();
    if input.len() != tokens.len() {
        return None;
    }
    let mut extracted = String::new();
    for (tok, &ch) in tokens.iter().zip(input.iter()) {
        if !tok.accepts(ch) {
            return None;
        }
        if tok.is_slot() {
            extracted.push(ch);
        }
    }
    Some(extracted)
}

// ── number picture clauses (`num{...}` — XFA 3.3 pp. 1160–1168) ─────────────────────────────

#[derive(Default)]
struct NumSpec {
    prefix_lit: String,
    suffix_lit: String,
    currency: Option<char>,
    currency_inside_parens: bool,
    paren_neg: bool,
    trailing_minus: bool,
    leading_plus: bool,
    trailing_plus: bool,
    cr_db: Option<String>,
    percent: bool,
    grouped: bool,
    min_int: usize,
    decimals: usize,
}

fn parse_num_spec(pattern: &str) -> NumSpec {
    let pat = strip_wrapper(pattern);
    let pic: Vec<char> = pat.chars().collect();
    let mut spec = NumSpec::default();
    let mut in_digits = false;
    let mut in_frac = false;
    let mut seen_open_paren = false;
    let mut i = 0;
    while let Some(&ch) = pic.get(i) {
        if ch == '\'' {
            let mut lit = String::new();
            i = skip_quoted_literal(&pic, i + 1, &mut Some(&mut lit));
            if in_digits {
                spec.suffix_lit.push_str(&lit);
            } else {
                spec.prefix_lit.push_str(&lit);
            }
            continue;
        }
        if ch == '\\' {
            if let Some(&next) = pic.get(i + 1) {
                if in_digits {
                    spec.suffix_lit.push(next);
                } else {
                    spec.prefix_lit.push(next);
                }
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if apply_cr_db(&pic, i, &mut spec) {
            i += 2;
            continue;
        }
        apply_num_symbol(ch, &mut in_digits, &mut in_frac, &mut seen_open_paren, &mut spec);
        i += 1;
    }
    spec.min_int = spec.min_int.min(20);
    spec.decimals = spec.decimals.min(20);
    spec
}

fn apply_cr_db(pic: &[char], i: usize, spec: &mut NumSpec) -> bool {
    let Some(&c0) = pic.get(i) else { return false };
    let Some(&c1) = pic.get(i + 1) else { return false };
    if (c0.eq_ignore_ascii_case(&'C') && c1.eq_ignore_ascii_case(&'R')) || (c0.eq_ignore_ascii_case(&'D') && c1.eq_ignore_ascii_case(&'B')) {
        let trailing_space = spec.suffix_lit.ends_with(' ');
        if trailing_space {
            spec.suffix_lit.pop();
            spec.cr_db = Some(format!(" {}{}", c0.to_ascii_uppercase(), c1.to_ascii_uppercase()));
        } else {
            spec.cr_db = Some(format!("{}{}", c0.to_ascii_uppercase(), c1.to_ascii_uppercase()));
        }
        return true;
    }
    false
}

fn apply_num_symbol(ch: char, in_digits: &mut bool, in_frac: &mut bool, seen_open_paren: &mut bool, spec: &mut NumSpec) {
    match ch {
        '9' => {
            *in_digits = true;
            if *in_frac {
                spec.decimals += 1;
            } else {
                spec.min_int += 1;
            }
        }
        'z' | 'Z' => {
            *in_digits = true;
            if *in_frac {
                spec.decimals += 1;
            }
        }
        '.' => {
            *in_digits = true;
            *in_frac = true;
        }
        ',' if *in_digits && !*in_frac => spec.grouped = true,
        '(' if !*in_digits => *seen_open_paren = true,
        ')' if *in_digits && *seen_open_paren => spec.paren_neg = true,
        '$' => {
            spec.currency = Some('$');
            spec.currency_inside_parens = *seen_open_paren;
        }
        '%' => spec.percent = true,
        '-' if *in_digits => spec.trailing_minus = true,
        '+' if !*in_digits => spec.leading_plus = true,
        '+' if *in_digits => spec.trailing_plus = true,
        's' | 'S' | '-' => {}
        other => {
            if *in_digits {
                spec.suffix_lit.push(other);
            } else {
                spec.prefix_lit.push(other);
            }
        }
    }
}

/// Format a number `num` according to an XFA number picture (`num{...}` or bare).
pub(super) fn format_num_pic(num: f64, pattern: &str) -> String {
    let first_alt = split_alternatives(pattern).into_iter().next().unwrap_or(pattern);
    let spec = parse_num_spec(first_alt);
    let scaled_num = if spec.percent { num * 100.0 } else { num };
    let width = spec.min_int + if spec.decimals > 0 { spec.decimals + 1 } else { 0 };
    let mut digits = format!("{:0>width$}", fixed(scaled_num.abs(), spec.decimals));
    if spec.grouped {
        digits = insert_thousands_commas(&digits);
    }
    let nonzero = digits.chars().any(|c| ('1'..='9').contains(&c));
    let is_neg = scaled_num < 0.0 && nonzero;
    let is_pos = scaled_num > 0.0 && nonzero;
    assemble_formatted_num(&spec, &digits, is_neg, is_pos)
}

fn insert_thousands_commas(text: &str) -> String {
    let (int, frac) = text.split_once('.').map_or((text, ""), |(a, b)| (a, b));
    if !int.bytes().all(|b| b.is_ascii_digit()) {
        return text.to_string();
    }
    let mut g = String::new();
    for (k, c) in int.chars().enumerate() {
        if k > 0 && (int.len() - k) % 3 == 0 {
            g.push(',');
        }
        g.push(c);
    }
    if frac.is_empty() { g } else { format!("{g}.{frac}") }
}

fn assemble_formatted_num(spec: &NumSpec, digits: &str, is_neg: bool, is_pos: bool) -> String {
    let mut core = digits.to_string();
    if let Some(curr) = spec.currency
        && (!spec.paren_neg || spec.currency_inside_parens || !is_neg)
    {
        core = format!("{curr}{core}");
    }
    if spec.paren_neg && is_neg {
        core = format!("({core})");
        if let Some(curr) = spec.currency
            && !spec.currency_inside_parens
        {
            core = format!("{curr}{core}");
        }
    } else if let Some(cr_db) = &spec.cr_db {
        if is_neg {
            core.push_str(cr_db);
        }
    } else if spec.trailing_minus {
        if is_neg {
            core.push('-');
        }
    } else if spec.trailing_plus {
        if is_neg {
            core.push('-');
        } else if is_pos {
            core.push('+');
        }
    } else if is_neg {
        core = format!("-{core}");
    } else if spec.leading_plus && is_pos {
        core = format!("+{core}");
    }
    if spec.percent {
        core.push('%');
    }
    format!("{}{core}{}", spec.prefix_lit, spec.suffix_lit)
}

/// Parse `text` according to an XFA number picture (`num{...}` or bare), returning `None` on non-numeric input.
pub(super) fn parse_num_pic(text: &str, pattern: &str) -> Option<Val> {
    let spec = parse_num_spec(pattern);
    let mut t = text.trim();
    if t.is_empty() {
        return None;
    }
    if !spec.prefix_lit.is_empty() {
        t = t.strip_prefix(spec.prefix_lit.as_str()).unwrap_or(t).trim();
    }
    if !spec.suffix_lit.is_empty() {
        t = t.strip_suffix(spec.suffix_lit.as_str()).unwrap_or(t).trim();
    }
    let (core, is_neg) = strip_num_affixes(t)?;
    let cleaned: String = core.chars().filter(|&c| c != ',').collect();
    if !cleaned.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut val = cleaned.parse::<f64>().ok().filter(|v| v.is_finite())?;
    if is_neg {
        val = -val.abs();
    }
    if spec.percent {
        let scaled = val / 100.0;
        val = format!("{scaled:.15e}").parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(scaled);
    }
    if val == 0.0 {
        val = 0.0;
    }
    Some(Val::Num(val))
}

fn strip_num_affixes(mut t: &str) -> Option<(&str, bool)> {
    let mut is_neg = false;
    let mut seen_sign = false;
    let mut seen_curr = false;
    let mut seen_pct = false;
    for _ in 0..4 {
        let prev = t;
        t = t.trim();
        if !seen_sign && let Some((rest, neg)) = strip_one_sign_marker(t) {
            seen_sign = true;
            is_neg |= neg;
            t = rest;
        }
        if !seen_curr && let Some(rest) = t.strip_prefix('$') {
            seen_curr = true;
            t = rest;
        }
        if !seen_pct && let Some(rest) = t.strip_suffix('%') {
            seen_pct = true;
            t = rest;
        }
        if t == prev {
            break;
        }
    }
    let core = t.trim();
    if seen_sign && (core.starts_with(['+', '-', '(']) || core.ends_with(['+', '-', ')']) || strip_cr_db_suffix(core).is_some()) {
        return None;
    }
    Some((core, is_neg))
}

fn strip_one_sign_marker(t: &str) -> Option<(&str, bool)> {
    if let Some(inner) = t.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        return Some((inner, true));
    }
    if let Some(rest) = strip_cr_db_suffix(t) {
        return Some((rest, true));
    }
    if let Some(rest) = t.strip_prefix('-').or_else(|| t.strip_suffix('-')) {
        return Some((rest, true));
    }
    if let Some(rest) = t.strip_prefix('+').or_else(|| t.strip_suffix('+')) {
        return Some((rest, false));
    }
    None
}

fn strip_cr_db_suffix(t: &str) -> Option<&str> {
    let upper = t.to_ascii_uppercase();
    if upper.ends_with("CR") || upper.ends_with("DB") { t.get(..t.len().saturating_sub(2)) } else { None }
}
