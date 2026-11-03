use super::*;

/// Acrobat's AFDate_Format(n) / AFTime_Format(n) presets.
pub const DATE_PRESETS: [&str; 14] = [
    "m/d",
    "m/d/yy",
    "mm/dd/yy",
    "mm/yy",
    "d-mmm",
    "d-mmm-yy",
    "dd-mmm-yy",
    "yy-mm-dd",
    "mmm-yy",
    "mmmm-yy",
    "mmm d, yyyy",
    "mmmm d, yyyy",
    "m/d/yy h:MM tt",
    "m/d/yy HH:MM",
];

pub const TIME_PRESETS: [&str; 4] = ["HH:MM", "h:MM tt", "HH:MM:ss", "h:MM:ss tt"];

// ── running them ────────────────────────────────────────────────────────────────────────────

/// AFMakeNumber: a number from typed text (currency symbols, group separators, spaces and a
/// trailing % ignored; a comma decimal separator accepted).
pub fn make_number(s: &str) -> Option<f64> {
    let t: String = s.trim().chars().filter(|c| !c.is_whitespace() && !matches!(c, '$' | '€' | '£' | '¥' | '\'' | '%')).collect();
    if t.is_empty() {
        return None;
    }
    let neg = t.starts_with('(') && t.ends_with(')');
    let t = t.trim_start_matches('(').trim_end_matches(')');
    // "1.234,56" or "1234,56": comma is the decimal separator when it comes last.
    let normalized = match (t.rfind(','), t.rfind('.')) {
        (Some(c), Some(d)) if c > d => t.replace('.', "").replace(',', "."),
        (Some(c), None) if t.len() - c - 1 != 3 => t.replace(',', "."),
        _ => t.replace(',', ""),
    };
    let v: f64 = normalized.parse().ok()?;
    v.is_finite().then_some(if neg { -v } else { v })
}

/// A number as JavaScript prints it (integers without a fraction).
pub fn js_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

fn group(int: &str, sep: char) -> String {
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// A number with `decimals` places in separator style `sep` (0 "1,234.56", 1 "1234.56",
/// 2 "1.234,56", 3 "1234,56", 4 "1'234.56").
fn number_text(v: f64, decimals: u8, sep: u8) -> String {
    // Round half away from zero, as JavaScript's toFixed does for these values.
    let p = 10f64.powi(i32::from(decimals));
    let scaled = v.abs() * p;
    // Past 2^53 every value is already whole, and scaling could overflow to infinity.
    let r = if scaled.is_finite() { scaled.round() / p } else { v.abs() };
    let s = format!("{:.*}", decimals as usize, r);
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(a, b)| (a, b));
    let (g, d) = match sep {
        1 => (None, '.'),
        2 => (Some('.'), ','),
        3 => (None, ','),
        4 => (Some('\''), '.'),
        _ => (Some(','), '.'),
    };
    let mut out = match g {
        Some(c) => group(int, c),
        None => int.to_string(),
    };
    if !frac.is_empty() {
        out.push(d);
        out.push_str(frac);
    }
    out
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// A date and time read from text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub y: i32,
    pub m: u32,
    pub d: u32,
    pub hh: u32,
    pub mm: u32,
    pub ss: u32,
}

/// `y` for chrono: `y` itself while chrono can represent it and the year after (`days_in` reads
/// the next January), and past that range `y` moved into 0..400. The Gregorian calendar repeats
/// every 400 years (146,097 days, a whole number of weeks), so month lengths and weekdays are the
/// same.
fn chrono_year(y: i32) -> i32 {
    let fits = |y: i32| chrono::NaiveDate::from_ymd_opt(y, 1, 1).is_some();
    if fits(y) && y.checked_add(1).is_some_and(fits) { y } else { y.rem_euclid(400) }
}

/// Number of days in month `m` (1-12) of year `y`, from chrono (the first of the next month less
/// the first of this one). An invalid month counts 31 days, as before.
fn days_in(y: i32, m: u32) -> u32 {
    let y = chrono_year(y);
    let first = |y: i32, m: u32| chrono::NaiveDate::from_ymd_opt(y, m, 1);
    let next = if m == 12 { first(y + 1, 1) } else { first(y, m.saturating_add(1)) };
    match (first(y, m), next) {
        (Some(a), Some(b)) => u32::try_from((b - a).num_days()).unwrap_or(31),
        _ => 31,
    }
}

/// The weekday (0 = Sunday) of `y-m-d`, from chrono; `None` for an invalid date (a month out of
/// range, or a day the month doesn't have).
fn weekday(y: i32, m: u32, d: u32) -> Option<usize> {
    use chrono::Datelike;
    let date = chrono::NaiveDate::from_ymd_opt(chrono_year(y), m, d)?;
    usize::try_from(date.weekday().num_days_from_sunday()).ok()
}

/// One field of a date format, in the order the format has them.
struct Part {
    /// `'y'`, `'m'`, `'d'`, `'H'`, `'M'` or `'s'`; a run of literal digits (the fixed year in
    /// "dd.mm.26") is `'#'`.
    kind: char,
    /// The value of a `'#'` run; 0 for the others.
    literal: u32,
    /// The digits the field takes when typed run together with the next one: 4 for "yyyy", 2 for
    /// "yy", "mm", "dd", "HH", "MM" and "ss". 0 when the width varies ("m", "d") or the field is
    /// a name ("mmm").
    width: usize,
    /// The next field follows with nothing between them ("yyyymmdd"), so one run of typed digits
    /// can hold both.
    joined: bool,
}

/// The fields of a format (year, month, day, hours, minutes, seconds and literal digits), in
/// order.
fn order(fmt: &str) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::new();
    // Weekday names (ddd, dddd) are shown, not typed: leave them out.
    let mut cleaned = fmt.replace("dddd", " ");
    cleaned = cleaned.replace("ddd", " ");
    let mut prev = ' ';
    // The letter of the last field's current run and its length so far.
    let mut run: Option<(char, usize)> = None;
    let fixed = |k: char, n: usize| match (k, n) {
        ('y', 2 | 4) | ('m' | 'd' | 'H' | 'M' | 's', 2) => n,
        _ => 0,
    };
    for c in cleaned.chars() {
        let continues = run.is_some_and(|(r, _)| r == c);
        if let Some((r, n)) = run
            && !continues
        {
            // The run ended; it was the last field's width.
            if let Some(last) = out.last_mut().filter(|p| p.kind != '#') {
                last.width = fixed(if r == 'h' { 'H' } else { r }, n);
            }
            run = None;
        }
        if let Some(digit) = c.to_digit(10) {
            match out.last_mut() {
                Some(p) if p.kind == '#' && prev.is_ascii_digit() => p.literal = p.literal.saturating_mul(10).saturating_add(digit),
                _ => out.push(Part { kind: '#', literal: digit, width: 0, joined: false }),
            }
            prev = c;
            continue;
        }
        let k = match c {
            'y' => 'y',
            'm' => 'm',
            'd' => 'd',
            'H' | 'h' => 'H',
            'M' => 'M',
            's' => 's',
            _ => {
                prev = c;
                continue;
            }
        };
        if continues {
            run = run.map(|(r, n)| (r, n.saturating_add(1)));
        } else {
            // A field starting right where the last one's letters stopped is joined to it.
            let adjacent = matches!(prev, 'y' | 'm' | 'd' | 'H' | 'h' | 'M' | 's');
            if out.last().map(|p| p.kind) != Some(k) {
                if let Some(last) = out.last_mut().filter(|p| p.kind != '#') {
                    last.joined = adjacent;
                }
                out.push(Part { kind: k, literal: 0, width: 0, joined: false });
            }
            run = Some((c, 1));
        }
        prev = c;
    }
    if let (Some((r, n)), Some(last)) = (run, out.last_mut().filter(|p| p.kind != '#')) {
        last.width = fixed(if r == 'h' { 'H' } else { r }, n);
    }
    out
}

/// Parse typed text against a date/time format (AFDate_KeystrokeEx's leniency: any
/// separators, month names, two-digit years). `None` when it isn't a valid date.
pub fn parse_date(text: &str, fmt: &str) -> Option<DateTime> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let lower = t.to_lowercase();
    // A bare trailing "p" ("5:30p") only counts when it follows a digit, not a word like "Sep".
    let bare_p = lower.strip_suffix('p').is_some_and(|rest| rest.trim_end().ends_with(|c: char| c.is_ascii_digit()));
    let pm = lower.contains("pm") || bare_p;
    let bare_a = lower.strip_suffix('a').is_some_and(|rest| rest.trim_end().ends_with(|c: char| c.is_ascii_digit()));
    let am = lower.contains("am") || bare_a;
    // Tokens: runs of digits (kept as text, so a run can be split between fields typed without
    // separators) and month names.
    let mut nums: VecDeque<String> = VecDeque::new();
    let mut month_name: Option<u32> = None;
    let mut cur = String::new();
    let flush = |cur: &mut String, nums: &mut VecDeque<String>, month: &mut Option<u32>| {
        if cur.is_empty() {
            return;
        }
        if cur.starts_with(|c: char| c.is_ascii_digit()) {
            nums.push_back(std::mem::take(cur));
        } else if cur.len() >= 3
            && let Some(i) = MONTHS.iter().position(|m| m.to_lowercase().starts_with(&cur.to_lowercase()))
        {
            *month = Some(i as u32 + 1);
        }
        cur.clear();
    };
    for c in t.chars() {
        // Digits and letters form separate tokens ("5Mar" is 5 and Mar).
        let same_kind = cur.is_empty() || cur.chars().next().is_some_and(|x| x.is_ascii_digit()) == c.is_ascii_digit();
        if c.is_alphanumeric() && same_kind {
            cur.push(c);
        } else {
            flush(&mut cur, &mut nums, &mut month_name);
            if c.is_alphanumeric() {
                cur.push(c);
            }
        }
    }
    flush(&mut cur, &mut nums, &mut month_name);
    let mut dt = DateTime { y: 0, m: 1, d: 1, hh: 0, mm: 0, ss: 0 };
    // The next number for a field. A field joined to the next one in the format ("yyyymmdd")
    // takes only its own width from a longer run of digits and leaves the rest for the next.
    // A run typed on its own is split only when fewer runs were typed than the fields of its kind
    // still to fill (`need`, this one included: date fields for a date field, time fields for a
    // time field, since times may be left out), and only when its rest can fill the next field
    // (`next_width`): with separators each run is one field's number, so "2026-10-18" keeps its
    // four-digit year in a yymmdd or yymmddHHMM field, and "2026 10 018" is the 18th. What is left
    // of a split run (`tail`: the front of `nums` is one) goes on being split along the fields
    // written side by side, and must fill its last field exactly, so "202610018" leaves "018" for
    // dd and is refused. `None` when there is no number left, it doesn't fit a u32, or such a
    // rest has the wrong width.
    let next_number = |nums: &mut VecDeque<String>, tail: &mut bool, part: &Part, need: usize, next_width: usize| -> Option<u32> {
        let run = nums.pop_front()?;
        let was_tail = std::mem::replace(tail, false);
        let short = nums.len() < need.saturating_sub(1);
        let digits = match (run.get(..part.width), run.get(part.width..)) {
            (Some(head), Some(rest)) if part.joined && part.width > 0 && !rest.is_empty() && (was_tail || (short && rest.len() >= next_width)) => {
                nums.push_front(rest.to_string());
                *tail = true;
                head
            }
            _ if was_tail && part.width > 0 && run.len() != part.width => return None,
            _ => run.as_str(),
        };
        digits.parse::<u32>().ok()
    };
    let parts = order(fmt);
    let takes_number = |p: &Part| matches!(p.kind, 'y' | 'd' | 'H' | 'M' | 's') || (p.kind == 'm' && month_name.is_none());
    let is_time = |p: &Part| matches!(p.kind, 'H' | 'M' | 's');
    // The fields of each kind still to fill: [date, time].
    let mut wanted = [parts.iter().filter(|p| takes_number(p) && !is_time(p)).count(), parts.iter().filter(|p| is_time(p)).count()];
    let mut tail = false;
    let mut have_y = false;
    for (i, part) in parts.iter().enumerate() {
        let kind = usize::from(is_time(part));
        let need = wanted.get(kind).copied().unwrap_or(0);
        let next_width = parts.get(i + 1).map_or(0, |p| p.width);
        if takes_number(part)
            && let Some(left) = wanted.get_mut(kind)
        {
            *left = left.saturating_sub(1);
        }
        let take = |nums: &mut VecDeque<String>, tail: &mut bool| next_number(nums, tail, part, need, next_width);
        match part.kind {
            // Literal digits in the format may be typed or left out ("15.05.26" or "15.05"
            // for "dd.mm.26"); a different number there is left over and rejected below.
            '#' if nums.front().and_then(|n| n.parse::<u32>().ok()) == Some(part.literal) => {
                nums.pop_front();
                tail = false;
            }
            'm' => {
                dt.m = match month_name {
                    Some(m) => m,
                    None => take(&mut nums, &mut tail)?,
                }
            }
            'd' => dt.d = take(&mut nums, &mut tail)?,
            'y' => {
                let y = i32::try_from(take(&mut nums, &mut tail)?).ok()?;
                dt.y = if y < 100 { if y < 50 { 2000 + y } else { 1900 + y } } else { y };
                have_y = true;
            }
            // Times may be left out; a number that is there must be readable.
            'H' => dt.hh = if nums.is_empty() { 0 } else { take(&mut nums, &mut tail)? },
            'M' => dt.mm = if nums.is_empty() { 0 } else { take(&mut nums, &mut tail)? },
            's' => dt.ss = if nums.is_empty() { 0 } else { take(&mut nums, &mut tail)? },
            _ => {}
        }
    }
    if !nums.is_empty() {
        return None;
    }
    if !have_y {
        dt.y = 2000;
    }
    if pm && dt.hh < 12 {
        dt.hh += 12;
    }
    if am && dt.hh == 12 {
        dt.hh = 0;
    }
    let has_date = fmt.contains(['y', 'm', 'd']);
    if has_date && (!(1..=12).contains(&dt.m) || dt.d == 0 || dt.d > days_in(dt.y, dt.m)) {
        return None;
    }
    if dt.hh > 23 || dt.mm > 59 || dt.ss > 59 {
        return None;
    }
    Some(dt)
}

/// Format a date/time with Acrobat's date format letters.
pub fn format_date(dt: DateTime, fmt: &str) -> String {
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut n = 1;
        while i + n < chars.len() && chars[i + n] == c {
            n += 1;
        }
        let h12 = if dt.hh.is_multiple_of(12) { 12 } else { dt.hh % 12 };
        match (c, n) {
            ('y', 4..) => {
                let _ = write!(out, "{:04}", dt.y);
            }
            ('y', _) => {
                let _ = write!(out, "{:02}", dt.y.rem_euclid(100));
            }
            ('m', 3..) => match MONTHS.get((dt.m as usize).wrapping_sub(1)) {
                Some(name) if n >= 4 => out.push_str(name),
                Some(name) => out.push_str(name.get(..3).unwrap_or(name)),
                None => {
                    let _ = write!(out, "{}", dt.m);
                }
            },
            ('m', 2) => {
                let _ = write!(out, "{:02}", dt.m);
            }
            ('m', _) => {
                let _ = write!(out, "{}", dt.m);
            }
            ('d', 3..) => match weekday(dt.y, dt.m, dt.d).and_then(|w| DAYS.get(w)) {
                Some(name) => out.push_str(if n >= 4 { name } else { name.get(..3).unwrap_or(name) }),
                // A day the month doesn't have has no weekday: it shows as its number.
                None => {
                    let _ = write!(out, "{}", dt.d);
                }
            },
            ('d', 2) => {
                let _ = write!(out, "{:02}", dt.d);
            }
            ('d', _) => {
                let _ = write!(out, "{}", dt.d);
            }
            ('H', 2) => {
                let _ = write!(out, "{:02}", dt.hh);
            }
            ('H', _) => {
                let _ = write!(out, "{}", dt.hh);
            }
            ('h', 2) => {
                let _ = write!(out, "{h12:02}");
            }
            ('h', _) => {
                let _ = write!(out, "{h12}");
            }
            ('M', 2) => {
                let _ = write!(out, "{:02}", dt.mm);
            }
            ('M', _) => {
                let _ = write!(out, "{}", dt.mm);
            }
            ('s', 2) => {
                let _ = write!(out, "{:02}", dt.ss);
            }
            ('s', _) => {
                let _ = write!(out, "{}", dt.ss);
            }
            ('t', 2) => out.push_str(if dt.hh < 12 { "am" } else { "pm" }),
            ('t', _) => out.push(if dt.hh < 12 { 'a' } else { 'p' }),
            (c, n) => out.extend(std::iter::repeat_n(c, n)),
        }
        i += n;
    }
    out
}

const SPECIAL_MASKS: [&str; 4] = ["99999", "99999-9999", "(999) 999-9999", "999-99-9999"];

fn mask_fits(mask: &str, text: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let m: Vec<char> = mask.chars().collect();
    t.len() == m.len()
        && m.iter().zip(&t).all(|(mc, tc)| match mc {
            '9' => tc.is_ascii_digit(),
            'A' => tc.is_alphabetic(),
            'O' => tc.is_alphanumeric(),
            'X' => true,
            other => other == tc,
        })
}

/// Fill a mask from the typed characters that fit its slots.
fn apply_mask(mask: &str, text: &str) -> Option<String> {
    let mut src = text.chars().filter(|c| c.is_alphanumeric());
    let mut out = String::new();
    for mc in mask.chars() {
        match mc {
            '9' | 'A' | 'O' | 'X' => {
                let c = src.next()?;
                let ok = match mc {
                    '9' => c.is_ascii_digit(),
                    'A' => c.is_alphabetic(),
                    _ => true,
                };
                if !ok {
                    return None;
                }
                out.push(c);
            }
            other => out.push(other),
        }
    }
    src.next().is_none().then_some(out)
}

/// The text shown for a value (the Format event).
pub fn format_value(f: &Format, value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    match f {
        Format::None => value.to_string(),
        Format::Number { decimals, sep, neg, currency, prepend } => {
            let Some(v) = make_number(value) else { return value.to_string() };
            let body = number_text(v, *decimals, *sep);
            let body = match (currency.is_empty(), *prepend) {
                (true, _) => body,
                (false, true) => format!("{currency}{body}"),
                (false, false) => format!("{body}{currency}"),
            };
            match (v < 0.0, neg) {
                (false, _) => body,
                // Styles 2 and 3 use parentheses (3 is also red, which the appearance can't
                // know here); 0 and 1 a minus sign.
                (true, 2 | 3) => format!("({body})"),
                (true, _) => format!("-{body}"),
            }
        }
        Format::Percent { decimals, sep } => match make_number(value) {
            Some(v) => format!(
                "{}%",
                if v < 0.0 { format!("-{}", number_text(v * 100.0, *decimals, *sep)) } else { number_text(v * 100.0, *decimals, *sep) }
            ),
            None => value.to_string(),
        },
        Format::Date(fmt) | Format::Time(fmt) => parse_date(value, fmt).map_or_else(|| value.to_string(), |dt| format_date(dt, fmt)),
        Format::Special(n) => apply_mask(SPECIAL_MASKS[(*n).min(3) as usize], value).unwrap_or_else(|| value.to_string()),
        Format::Mask(m) => apply_mask(m, value).unwrap_or_else(|| value.to_string()),
    }
}

/// The Keystroke event on commit: does the value fit the format? Returns the value to store
/// (numbers are normalised), or Acrobat's message.
pub fn keystroke(f: &Format, field: &str, value: &str) -> Result<String, String> {
    let bad = || format!("The value entered does not match the format of the field [ {field} ]");
    // Blank input clears a formatted field; without a format, whitespace is text like any other.
    if *f != Format::None && value.trim().is_empty() {
        return Ok(String::new());
    }
    match f {
        Format::None => Ok(value.to_string()),
        Format::Number { .. } => make_number(value).map(js_number).ok_or_else(bad),
        Format::Percent { .. } => {
            // Typed "12.5%" or "12.5" both mean 0.125 once stored? Acrobat stores the number
            // as typed and shows it ×100: keep the typed number.
            make_number(value).map(js_number).ok_or_else(bad)
        }
        Format::Date(fmt) | Format::Time(fmt) => parse_date(value, fmt)
            .map(|_| value.trim().to_string())
            .ok_or_else(|| format!("Invalid date/time: please ensure that the date/time exists. Field [ {field} ] should match format {fmt}")),
        Format::Special(n) => {
            let m = SPECIAL_MASKS[(*n).min(3) as usize];
            apply_mask(m, value).filter(|s| mask_fits(m, s)).map(|_| value.trim().to_string()).ok_or_else(bad)
        }
        Format::Mask(m) => apply_mask(m, value).map(|_| value.trim().to_string()).ok_or_else(bad),
    }
}

/// The Validate event: `Err` with Acrobat's message when the value is out of range.
pub fn validate(v: &Validate, value: &str) -> Result<(), String> {
    let Validate::Range { min, max } = v else { return Ok(()) };
    if value.trim().is_empty() {
        return Ok(());
    }
    let Some(x) = make_number(value) else { return Ok(()) };
    let fmt = |n: f64| js_number(n);
    let ok = min.is_none_or(|m| x >= m) && max.is_none_or(|m| x <= m);
    if ok {
        return Ok(());
    }
    Err(match (min, max) {
        (Some(a), Some(b)) => format!("Invalid value: must be greater than or equal to {} and less than or equal to {}.", fmt(*a), fmt(*b)),
        (Some(a), None) => format!("Invalid value: must be greater than or equal to {}.", fmt(*a)),
        (None, Some(b)) => format!("Invalid value: must be less than or equal to {}.", fmt(*b)),
        (None, None) => String::new(),
    })
}

#[cfg(test)]
mod calendar_tests {
    use super::{DateTime, chrono_year, days_in, format_date, parse_date, weekday};
    use chrono::Datelike;

    fn day(y: i32, m: u32, d: u32) -> DateTime {
        DateTime { y, m, d, hh: 0, mm: 0, ss: 0 }
    }

    /// Leap years follow the Gregorian rules: every 4th year, but not every 100th, except every
    /// 400th.
    #[test]
    fn typed_dates_follow_the_leap_year_rules() {
        assert!(parse_date("2000-02-29", "yyyy-mm-dd").is_some());
        assert!(parse_date("1900-02-29", "yyyy-mm-dd").is_none());
        assert!(parse_date("1900-03-01", "yyyy-mm-dd").is_some());
        assert!(parse_date("2100-02-28", "yyyy-mm-dd").is_some());
        assert!(parse_date("2100-02-29", "yyyy-mm-dd").is_none());
        assert!(parse_date("2024-02-29", "yyyy-mm-dd").is_some());
        assert!(parse_date("2024-02-31", "yyyy-mm-dd").is_none());
        assert!(parse_date("2026-04-31", "yyyy-mm-dd").is_none());
    }

    #[test]
    fn weekdays_come_from_the_calendar() {
        assert_eq!(format_date(day(2026, 11, 2), "dddd"), "Monday");
        assert_eq!(format_date(day(2000, 2, 29), "dddd"), "Tuesday");
        assert_eq!(format_date(day(1900, 3, 1), "ddd"), "Thu");
        assert_eq!(format_date(day(2100, 2, 28), "dddd"), "Sunday");
        // Past chrono's range the 400-year cycle gives the same weekday: 2,000,000,000 is a whole
        // number of cycles after 2000, and 2000-01-01 was a Saturday.
        assert_eq!(format_date(day(2_000_000_000, 1, 1), "dddd"), "Saturday");
        // i32::MIN.rem_euclid(400) is 352, and 352-01-01 was a Tuesday.
        assert_eq!(format_date(day(i32::MIN, 1, 1), "dddd"), "Tuesday");
        // At the edge of chrono's range, chrono's own answer and the cycle's agree.
        let last = chrono::NaiveDate::MAX.year();
        for y in [last - 1, last, last + 1, -last - 1, -last] {
            assert_eq!(chrono_year(y).rem_euclid(400), y.rem_euclid(400));
            for m in 1..=12 {
                assert_eq!(days_in(y, m), days_in(y.rem_euclid(400), m), "{y}-{m}");
                assert_eq!(weekday(y, m, 1), weekday(y.rem_euclid(400), m, 1), "{y}-{m}");
            }
        }
        assert_eq!(chrono_year(2026), 2026, "a year chrono represents is passed through");
        // Year 0 began on a Saturday. Sakamoto's method with truncating division put January and
        // February of year 0 (and of negative years) one day late.
        assert_eq!(format_date(day(0, 1, 1), "dddd"), "Saturday");
        assert_eq!(format_date(day(0, 2, 29), "dddd"), "Tuesday");
    }

    /// `DateTime` is public, so it can hold a day its month doesn't have. That day has no weekday
    /// and shows as its number, not as the weekday of some other date.
    #[test]
    fn an_impossible_day_shows_its_number_not_a_weekday() {
        assert_eq!(format_date(day(2026, 2, 30), "dddd d mmmm"), "30 30 February");
        assert_eq!(format_date(day(2025, 2, 29), "ddd"), "29");
        assert_eq!(format_date(day(2024, 13, 1), "dddd"), "1");
    }
}
