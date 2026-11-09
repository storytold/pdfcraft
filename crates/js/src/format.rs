use super::host::{arg, error, s, text};
use super::run::{MAX_PRINTF_OUTPUT_BYTES, MAX_PRINTF_WIDTH_PRECISION};
use super::{Context, JsDate, JsResult, JsString, JsValue, formcalc};

/// `util.printf(cFormat, …)`: `%[,n][+ 0#][width][.prec](d|f|s|x)`, where `,0` groups
/// thousands with commas (the default), `,1` doesn't, `,2` uses `.` and `,`, `,3` uses `.`
/// without grouping.
pub(crate) fn printf(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let fmt = text(&arg(args, 0), ctx)?;
    let mut out = String::new();
    let check_output = |len: usize| -> JsResult<()> {
        if len > MAX_PRINTF_OUTPUT_BYTES {
            return Err(error(format!("util.printf output exceeds {MAX_PRINTF_OUTPUT_BYTES} bytes")));
        }
        Ok(())
    };
    let mut next = 1;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            check_output(out.len().saturating_add(c.len_utf8()))?;
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            check_output(out.len().saturating_add(1))?;
            out.push('%');
            continue;
        }
        let mut sep = 1u8;
        if chars.peek() == Some(&',') {
            chars.next();
            sep = chars.next().and_then(|d| d.to_digit(10)).unwrap_or(0) as u8;
        }
        let (mut plus, mut space, mut zero) = (false, false, false);
        while let Some(&f) = chars.peek() {
            match f {
                '+' => plus = true,
                ' ' => space = true,
                '0' => zero = true,
                '#' => {}
                _ => break,
            }
            chars.next();
        }
        let mut width = 0usize;
        while let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
            width = width.saturating_mul(10).saturating_add(d as usize);
            chars.next();
        }
        let mut prec = None;
        if chars.peek() == Some(&'.') {
            chars.next();
            let mut p = 0usize;
            while let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
                p = p.saturating_mul(10).saturating_add(d as usize);
                chars.next();
            }
            prec = Some(p);
        }
        if width > MAX_PRINTF_WIDTH_PRECISION || prec.is_some_and(|p| p > MAX_PRINTF_WIDTH_PRECISION) {
            return Err(error(format!("util.printf width or precision exceeds {MAX_PRINTF_WIDTH_PRECISION}")));
        }
        let conv = chars.next().unwrap_or('s');
        let v = arg(args, next);
        next += 1;
        let body = match conv {
            'd' | 'f' => {
                let n = v.to_number(ctx)?;
                let p = if conv == 'd' { 0 } else { prec.unwrap_or(6) };
                let n = if conv == 'd' { n.trunc() } else { n };
                let mut t = group(&format!("{:.*}", p, n.abs()), sep);
                if n < 0.0 {
                    t.insert(0, '-');
                } else if plus {
                    t.insert(0, '+');
                } else if space {
                    t.insert(0, ' ');
                }
                t
            }
            'x' => format!("{:X}", v.to_number(ctx)? as i64),
            _ => {
                let t = text(&v, ctx)?;
                match prec {
                    Some(p) => t.chars().take(p).collect(),
                    None => t,
                }
            }
        };
        let pad = width.saturating_sub(body.chars().count());
        check_output(out.len().saturating_add(pad).saturating_add(body.len()))?;
        if zero && matches!(conv, 'd' | 'f') {
            let (sign, digits) = if body.starts_with(['-', '+', ' ']) { body.split_at(1) } else { ("", body.as_str()) };
            out.push_str(sign);
            out.push_str(&"0".repeat(pad));
            out.push_str(digits);
        } else {
            out.push_str(&" ".repeat(pad));
            out.push_str(&body);
        }
    }
    Ok(s(&out))
}

/// Group the integer part of a plain decimal number per `util.printf`'s `,n` flag.
fn group(t: &str, sep: u8) -> String {
    let (int, frac) = t.split_once('.').map_or((t, None), |(a, b)| (a, Some(b)));
    let (thousands, point) = match sep {
        0 => (Some(','), '.'),
        2 => (Some('.'), ','),
        3 => (None, ','),
        _ => (None, '.'),
    };
    let mut g = String::new();
    for (i, ch) in int.chars().enumerate() {
        if let Some(th) = thousands
            && i > 0
            && (int.len() - i).is_multiple_of(3)
        {
            g.push(th);
        }
        g.push(ch);
    }
    if let Some(f) = frac {
        g.push(point);
        g.push_str(f);
    }
    g
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// Month and weekday names (weekdays from Sunday): full, then short.
type Names = ([&'static str; 12], [&'static str; 12], [&'static str; 7], [&'static str; 7]);

const ENGLISH: Names = (
    MONTHS,
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
    DAYS,
    ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
);

static NAMES: std::sync::RwLock<Names> = std::sync::RwLock::new(ENGLISH);

/// Sets the month and weekday names `util.printd` writes for `mmmm`, `mmm`, `dddd` and `ddd`
/// (weekdays from Sunday), as a localized Acrobat writes its own language's (#5303).
/// `util.scand` reads them as well as English.
pub fn set_date_names(months: [&'static str; 12], months_short: [&'static str; 12], days: [&'static str; 7], days_short: [&'static str; 7]) {
    *NAMES.write().unwrap_or_else(std::sync::PoisonError::into_inner) = (months, months_short, days, days_short);
}

/// `text` with the month and weekday names of `lists` (in the order months, short months,
/// weekdays, short weekdays) written as their English names, so a parser that reads English
/// reads them. The longest name wins at each place, case ignored, and only a whole name: one
/// that doesn't continue a word, nor end in a digit that a number continues. A name may hold
/// digits ("10月"), spaces ("tháng 10"), dots ("ม.ค.") or combining vowel signs, which a
/// word-by-word reader would split (#5303). Where a weekday and a month share a name (Italian
/// "mar"), the order of the format's name fields (`mmm`, `ddd`) decides. English names are
/// found too, so that one typed ("Tue" before "Mar") fills its field of the format as a
/// localized one does.
fn english_names(text: &str, fmt: &str, lists: [&[&str]; 4]) -> String {
    const EN: [&[&str]; 4] = [&MONTHS, &MONTHS, &DAYS, &DAYS];
    // No date is this long: hostile text is left as it is rather than scanned name by name.
    if text.len() > MAX_DATE_TEXT_BYTES {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let typed_english: [&[&str]; 4] = [&ENGLISH.0, &ENGLISH.1, &ENGLISH.2, &ENGLISH.3];
    // Each name as folded chars, whether it is a month, and its English name.
    let mut names: Vec<(Vec<String>, bool, &str)> = Vec::new();
    for (k, (list, en)) in lists.into_iter().zip(EN).chain(typed_english.into_iter().zip(EN)).enumerate() {
        for (name, english) in list.iter().zip(en.iter()) {
            // A short name ending in a dot ("mar.") is also read typed without it, as the same
            // kind of name, so that where it equals another ("Mar", March) the format decides.
            let bare = name.trim_end_matches('.');
            for name in std::iter::once(*name).chain((bare.len() < name.len()).then_some(bare)) {
                let n: Vec<String> = name.chars().map(fold).collect();
                if !n.is_empty() && n.len() <= MAX_NAME_CHARS {
                    names.push((n, k % 4 < 2, english));
                }
            }
        }
    }
    names.sort_by_key(|(n, ..)| std::cmp::Reverse(n.len()));
    // The format's name fields in order: true for a month, false for a weekday.
    let f: Vec<char> = fmt.chars().collect();
    let mut expected = Vec::new();
    let mut j = 0;
    while let Some(&c) = f.get(j) {
        let run = f.get(j..).map_or(1, |r| r.iter().take_while(|x| **x == c).count()).max(1);
        if run >= 3 && (c == 'm' || c == 'd') {
            expected.push(c == 'm');
        }
        j = j.saturating_add(run);
    }
    let mut slot = 0;
    // A name's first letter may not continue a word; its last may not run on into a letter, nor
    // (a digit) into a number.
    let letter = |c: Option<&char>| c.is_some_and(|c| c.is_alphabetic());
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let fits = |n: &Vec<String>| {
            let end = i.saturating_add(n.len());
            let first = n.first().and_then(|e| e.chars().next());
            let last = n.last().and_then(|e| e.chars().next());
            chars.get(i..end).is_some_and(|t| t.iter().map(|c| fold(*c)).eq(n.iter().cloned()))
                && !(first.is_some_and(char::is_alphabetic) && letter(i.checked_sub(1).and_then(|p| chars.get(p))))
                && !match last {
                    Some(l) if l.is_ascii_digit() => chars.get(end).is_some_and(char::is_ascii_digit),
                    Some(l) if l.is_alphabetic() => letter(chars.get(end)),
                    _ => false,
                }
        };
        let longest = names.iter().find(|(n, ..)| fits(n)).map(|(n, ..)| n.len());
        let hit = longest.and_then(|len| {
            let mut same = names.iter().filter(|(n, ..)| n.len() == len && fits(n));
            let want = expected.get(slot).copied();
            let all: Vec<_> = same.by_ref().collect();
            all.iter().find(|(_, month, _)| Some(*month) == want).or_else(|| all.first()).copied()
        });
        match hit {
            Some((n, month, english)) => {
                if expected.get(slot) == Some(month) {
                    slot += 1;
                }
                // Apart from what touches it ("10月18"), but no space added at either end.
                if out.ends_with(char::is_alphanumeric) {
                    out.push(' ');
                }
                out.push_str(english);
                i = i.saturating_add(n.len());
                if chars.get(i).is_some_and(|c| c.is_alphanumeric()) {
                    out.push(' ');
                }
            }
            None => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// `c` as names are compared: lower case, with the capital I and the Turkish dotted and dotless
/// i all "i", and a combining dot above (U+0307) dropped, so that "EKİM", "SALI" and "MAYIS" are
/// Turkish "Ekim", "Salı" and "Mayıs", which lower-casing alone makes "ekim" with a dotted i,
/// "sali" and "mayis" (#5303). The Greek final sigma is a sigma, since a capital Σ, which
/// lower-cases to σ on its own, ends "ΙΑΝΟΥΆΡΙΟΣ" for "Ιανουάριος". One piece for each char, so
/// positions in the text stay aligned.
fn fold(c: char) -> String {
    match c {
        'I' | 'ı' | 'İ' => "i".to_string(),
        'ς' => "σ".to_string(),
        _ => c.to_lowercase().filter(|&l| l != '\u{307}').collect(),
    }
}

/// `s` [`fold`]ed char by char.
fn folded(s: &str) -> String {
    s.chars().map(fold).collect()
}

/// Typed dates longer than this are not searched for names.
const MAX_DATE_TEXT_BYTES: usize = 1024;

/// Names longer than this are not looked for (settings are trusted, but a cap keeps the scan cheap).
const MAX_NAME_CHARS: usize = 32;

fn names() -> Names {
    *NAMES.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

std::thread_local! {
    /// Whether `util.printd` on this thread writes English names, whatever the date language.
    static PRINTD_ENGLISH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `run` with `util.printd` writing English month and weekday names when `english` (scripts
/// it starts carry this to their own thread): for an appearance whose fonts can't draw the date
/// language's names, which would come out as `?` (#5303). `util.scand` still reads both.
pub fn with_english_printd<R>(english: bool, run: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            PRINTD_ENGLISH.with(|e| e.set(self.0));
        }
    }
    let _restore = Restore(PRINTD_ENGLISH.with(|e| e.replace(english)));
    run()
}

/// Whether [`with_english_printd`] is on for this thread.
pub(crate) fn printd_english() -> bool {
    PRINTD_ENGLISH.with(std::cell::Cell::get)
}

/// The names `util.printd` writes.
fn printd_names() -> Names {
    if printd_english() { ENGLISH } else { names() }
}

/// `util.printd(cFormat, oDate)`: yyyy yy mmmm mmm mm m dddd ddd dd d HH H hh h MM M ss s tt,
/// and the numeric formats 0 (D:yyyymmddHHMMss), 1 (yyyy.mm.dd HH:MM:ss) and 2 (m/d/yy h:MM:ss tt).
pub(crate) fn printd(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let f = arg(args, 0);
    let fmt = if f.is_number() {
        match f.to_number(ctx)? as i32 {
            0 => "D:yyyymmddHHMMss".to_string(),
            1 => "yyyy.mm.dd HH:MM:ss".to_string(),
            _ => "m/d/yy h:MM:ss tt".to_string(),
        }
    } else {
        text(&f, ctx)?
    };
    let d = arg(args, 1);
    let Some(date) = d.as_object() else { return Err(error("util.printd needs a Date")) };
    let mut part = |m: &str| -> JsResult<i64> {
        let func = date.get(JsString::from(m), ctx)?;
        let func = func.as_callable().ok_or_else(|| error("util.printd needs a Date"))?;
        let v = func.call(&d, &[], ctx)?.to_number(ctx)?;
        if v.is_nan() {
            return Err(error("invalid date"));
        }
        Ok(v as i64)
    };
    let (y, mo, day, wd, h, mi, sec) =
        (part("getFullYear")?, part("getMonth")?, part("getDate")?, part("getDay")?, part("getHours")?, part("getMinutes")?, part("getSeconds")?);
    let mut out = String::new();
    let (months, months_short, days, days_short) = printd_names();
    let b = fmt.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        // Only date letters read as runs; a literal is copied one character at a time, so
        // measuring its run would rescan a long literal stretch at every position.
        let run = if b"ymdHhMst".contains(&c) { b[i..].iter().take_while(|x| **x == c).count() } else { 1 };
        #[cfg(test)]
        PRINTD_SCANNED.fetch_add(run as u64, std::sync::atomic::Ordering::Relaxed);
        let push_num = |out: &mut String, v: i64, n: usize| {
            if n >= 2 {
                out.push_str(&format!("{v:02}"));
            } else {
                out.push_str(&v.to_string());
            }
        };
        match c {
            b'y' => {
                if run >= 4 {
                    out.push_str(&format!("{y:04}"));
                } else {
                    out.push_str(&format!("{:02}", y % 100));
                }
            }
            b'm' => match run {
                r if r >= 4 => out.push_str(months.get(mo.rem_euclid(12) as usize).copied().unwrap_or_default()),
                3 => out.push_str(months_short.get(mo.rem_euclid(12) as usize).copied().unwrap_or_default()),
                n => push_num(&mut out, mo + 1, n),
            },
            b'd' => match run {
                r if r >= 4 => out.push_str(days.get(wd.rem_euclid(7) as usize).copied().unwrap_or_default()),
                3 => out.push_str(days_short.get(wd.rem_euclid(7) as usize).copied().unwrap_or_default()),
                n => push_num(&mut out, day, n),
            },
            b'H' => push_num(&mut out, h, run),
            b'h' => push_num(&mut out, if h % 12 == 0 { 12 } else { h % 12 }, run),
            b'M' => push_num(&mut out, mi, run),
            b's' => push_num(&mut out, sec, run),
            b't' => {
                let ampm = if h < 12 { "am" } else { "pm" };
                out.push_str(if run >= 2 { ampm } else { &ampm[..1] });
            }
            b'\\' if i + 1 < b.len() => {
                // The escaped character may be multi-byte; copy it whole.
                let ch = fmt[i + 1..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += 1 + ch.len_utf8();
                continue;
            }
            _ => {
                // Copy the rest of a multi-byte character too.
                let ch = fmt[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += ch.len_utf8();
                continue;
            }
        }
        i += run;
    }
    Ok(s(&out))
}

/// A field of a `util.scand` format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DateUnit {
    /// `yyyy`: four digits.
    Year,
    /// `yy`: two digits, read as 1950–2049.
    ShortYear,
    Month,
    /// `mmm`, `mmmm`: a month's name or its three-letter abbreviation.
    MonthName,
    Day,
    /// `ddd`, `dddd`: a weekday's name, read and not checked.
    Weekday,
    Hour,
    /// `h`, `hh`: 0–12, with `tt` for the afternoon.
    Hour12,
    Minute,
    Second,
    /// `t`, `tt`: am or pm.
    AmPm,
}

impl DateUnit {
    /// A year, month or day: a format needs one of these for `util.scand` to read anything.
    fn is_date(self) -> bool {
        matches!(self, Self::Year | Self::ShortYear | Self::Month | Self::MonthName | Self::Day)
    }

    /// Read from a run of digits, not from letters.
    fn is_numeric(self) -> bool {
        !matches!(self, Self::MonthName | Self::Weekday | Self::AmPm)
    }
}

/// A piece of a `util.scand` format.
#[derive(Debug, PartialEq, Eq)]
enum ScanPart {
    /// A field, the digits it takes when typed together with the next one (`width`), and whether
    /// the next one follows it with nothing between them in the format (`joined`).
    Field { unit: DateUnit, width: usize, joined: bool },
    /// Letters the format writes as they are, which the text must have there.
    Word(String),
}

/// The fields and literal words of a `util.scand` format, in order. A numeric field is marked
/// `joined` when another numeric field follows it directly; anything but letters and digits
/// outside a field is left out.
fn scan_parts(fmt: &str) -> Vec<ScanPart> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut parts: Vec<ScanPart> = Vec::new();
    // The last part is a field written right before this point, or letters still being collected.
    let (mut after_field, mut in_word) = (false, false);
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        // Counted for field letters only, so a long run of anything else is read once.
        let run = if "ymdHhMst".contains(c) { chars.get(i..).unwrap_or_default().iter().take_while(|x| **x == c).count() } else { 1 };
        let field = match (c, run) {
            ('y', 4..) => Some((DateUnit::Year, 4)),
            ('y', _) => Some((DateUnit::ShortYear, 2)),
            ('m', 3..) => Some((DateUnit::MonthName, 0)),
            ('m', n) => Some((DateUnit::Month, n)),
            ('d', 3..) => Some((DateUnit::Weekday, 0)),
            ('d', n) => Some((DateUnit::Day, n)),
            ('H', n) => Some((DateUnit::Hour, n.min(2))),
            ('h', n) => Some((DateUnit::Hour12, n.min(2))),
            ('M', n) => Some((DateUnit::Minute, n.min(2))),
            ('s', n) => Some((DateUnit::Second, n.min(2))),
            ('t', _) => Some((DateUnit::AmPm, 0)),
            _ => None,
        };
        if let Some((unit, width)) = field {
            if after_field
                && unit.is_numeric()
                && let Some(ScanPart::Field { unit: last, joined, .. }) = parts.last_mut()
                && last.is_numeric()
            {
                *joined = true;
            }
            parts.push(ScanPart::Field { unit, width, joined: false });
            (after_field, in_word) = (true, false);
            i += run;
            continue;
        }
        // `\` writes the next character as it is.
        let (literal, step) = if c == '\\' { (chars.get(i + 1).copied(), 2) } else { (Some(c), 1) };
        i += step;
        after_field = false;
        match literal {
            Some(l) if l.is_alphanumeric() => match parts.last_mut() {
                Some(ScanPart::Word(w)) if in_word => w.push(l),
                _ => {
                    parts.push(ScanPart::Word(l.to_string()));
                    in_word = true;
                }
            },
            _ => in_word = false,
        }
    }
    parts
}

/// A run of digits or of letters in the text `util.scand` reads.
enum ScanToken<'a> {
    Digits(&'a str),
    Letters(&'a str),
}

/// The text's runs of digits and of letters, in order; anything else only separates them, and
/// may not begin or end the text. `None` when it does.
fn scan_tokens(text: &str) -> Option<Vec<ScanToken<'_>>> {
    // Some(true) for a digit, Some(false) for a letter, None for a separator.
    let kind = |c: char| (c.is_ascii_digit() || c.is_alphabetic()).then_some(c.is_ascii_digit());
    kind(text.chars().next()?)?;
    kind(text.chars().next_back()?)?;
    let mut out = Vec::new();
    let mut start: Option<(usize, bool)> = None;
    for (at, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        let k = kind(c);
        if let Some((from, digits)) = start
            && k != Some(digits)
        {
            let piece = text.get(from..at)?;
            out.push(if digits { ScanToken::Digits(piece) } else { ScanToken::Letters(piece) });
            start = None;
        }
        if let Some(digits) = k
            && start.is_none()
        {
            start = Some((at, digits));
        }
    }
    Some(out)
}

/// What `util.scand` reads.
#[derive(Debug, PartialEq, Eq)]
enum Scanned {
    /// The text is empty: today, at noon.
    Today,
    /// A date and time: year, month (1–12), day, hours, minutes, seconds.
    At(i32, u32, u32, u32, u32, u32),
}

/// The fields `util.scand` has read so far.
#[derive(Default)]
struct ScanValues {
    y: Option<i32>,
    mo: Option<u32>,
    d: Option<u32>,
    h: Option<u32>,
    mi: Option<u32>,
    sec: Option<u32>,
    pm: Option<bool>,
    /// The hour was read from `h` or `hh`.
    twelve: bool,
}

impl ScanValues {
    /// Records `v` as the value of `unit`: a `yy` year as 1950–2049, and an `h` or `hh` hour
    /// marked as 12-hour. Month names, weekdays and am/pm are not read here.
    fn set(&mut self, unit: DateUnit, v: u32) {
        match unit {
            DateUnit::Year => self.y = i32::try_from(v).ok(),
            DateUnit::ShortYear => self.y = i32::try_from(v).ok().map(|v| if v < 50 { 2000 + v } else { 1900 + v }),
            DateUnit::Month => self.mo = Some(v),
            DateUnit::Day => self.d = Some(v),
            DateUnit::Hour => self.h = Some(v),
            DateUnit::Hour12 => {
                self.h = Some(v);
                self.twelve = true;
            }
            DateUnit::Minute => self.mi = Some(v),
            DateUnit::Second => self.sec = Some(v),
            DateUnit::MonthName | DateUnit::Weekday | DateUnit::AmPm => {}
        }
    }
}

/// Reads `text` by a `util.scand` format, as another viewer was observed to (black-box):
///
/// - Fields may be separated by anything but digits and letters, whether or not the format
///   separates them, but nothing may come before the first or after the last.
/// - Digits typed together fill the format's fields written together with exactly as many
///   digits as each has letters (`yyyymd`: 4, 1 and 1). A run of digits for one field alone may
///   have one or two, except a four-digit year.
/// - `yy` is 1950–2049; month names may be written in full or as three letters, in any case.
/// - Impossible dates are refused, but a month or day of 0 is read as 1.
/// - A format with no year, month or day reads nothing; an empty text is today, at noon.
///
/// Not observed: a year left out is `this_year`, and a month or day left out is 1.
fn scan_date(fmt: &str, text: &str, this_year: i32) -> Option<Scanned> {
    let parts = scan_parts(fmt);
    if !parts.iter().any(|p| matches!(p, ScanPart::Field { unit, .. } if unit.is_date())) {
        return None;
    }
    if text.is_empty() {
        return Some(Scanned::Today);
    }
    let tokens = scan_tokens(text)?;
    let mut v = ScanValues::default();
    let (mut p, mut t) = (0, 0);
    while let Some(part) = parts.get(p) {
        let token = tokens.get(t)?;
        t += 1;
        match (part, token) {
            (ScanPart::Word(w), ScanToken::Letters(l) | ScanToken::Digits(l)) => {
                if w.to_lowercase() != l.to_lowercase() {
                    return None;
                }
                p += 1;
            }
            (ScanPart::Field { unit: DateUnit::MonthName, .. }, ScanToken::Letters(l)) => {
                let (months, months_short, ..) = names();
                v.mo = Some(named(&months, l).or_else(|| named(&months_short, l)).or_else(|| named(&MONTHS, l))?.saturating_add(1));
                p += 1;
            }
            (ScanPart::Field { unit: DateUnit::Weekday, .. }, ScanToken::Letters(l)) => {
                let (.., days, days_short) = names();
                named(&days, l).or_else(|| named(&days_short, l)).or_else(|| named(&DAYS, l))?;
                p += 1;
            }
            (ScanPart::Field { unit: DateUnit::AmPm, .. }, ScanToken::Letters(l)) => {
                v.pm = Some(match l.to_lowercase().as_str() {
                    "am" | "a" => false,
                    "pm" | "p" => true,
                    _ => return None,
                });
                p += 1;
            }
            (ScanPart::Field { unit, width, joined }, ScanToken::Digits(run)) if unit.is_numeric() => {
                // The fields written together from here, no more than the run's digits can fill,
                // and the fewest of them the run fills.
                let mut group = vec![(*unit, *width)];
                let (mut more, mut widths) = (*joined, *width);
                while more
                    && widths < run.len()
                    && let Some(ScanPart::Field { unit, width, joined }) = parts.get(p + group.len())
                {
                    group.push((*unit, *width));
                    widths = widths.saturating_add(*width);
                    more = *joined;
                }
                let mut sum = 0;
                let fills = group.iter().position(|(_, w)| {
                    sum += w;
                    sum == run.len()
                });
                if let Some(last) = fills {
                    let mut at = 0;
                    for (unit, w) in group.iter().take(last + 1) {
                        v.set(*unit, run.get(at..at + w)?.parse().ok()?);
                        at += w;
                    }
                    p += last + 1;
                } else if *unit != DateUnit::Year && run.len() <= 2 {
                    v.set(*unit, run.parse().ok()?);
                    p += 1;
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    }
    if t != tokens.len() {
        return None;
    }
    let y = v.y.unwrap_or(this_year);
    let mo = v.mo.unwrap_or(1).max(1);
    let d = v.d.unwrap_or(1).max(1);
    if mo > 12 || i64::from(d) > i64::from(formcalc::days_in(i64::from(y), mo)) {
        return None;
    }
    let mut h = v.h.unwrap_or(0);
    if v.twelve && h > 12 {
        return None;
    }
    match v.pm {
        Some(true) if h < 12 => h += 12,
        Some(false) if h == 12 => h = 0,
        _ => {}
    }
    let (mi, sec) = (v.mi.unwrap_or(0), v.sec.unwrap_or(0));
    (h <= 23 && mi <= 59 && sec <= 59).then_some(Scanned::At(y, mo, d, h, mi, sec))
}

/// The index of the name `word` is, in full or as its first three letters, in any case.
fn named(names: &[&str], word: &str) -> Option<u32> {
    let word = folded(word);
    let i = names.iter().position(|n| {
        let n = folded(n);
        // A short name may end in a dot ("Okt."); three letters may be cut from any name.
        word == n || word == n.trim_end_matches('.') || n.chars().take(3).collect::<String>() == word
    })?;
    u32::try_from(i).ok()
}

/// `util.scand(cFormat, cDate)`: the date `cDate` gives by a `util.printd` format, as a local
/// `Date`, or `null` when it doesn't read as one (see [`scan_date`]). Only text is read.
pub(crate) fn scand(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(typed) = arg(args, 1).as_string().map(|t| t.to_std_string_escaped()) else {
        return Ok(JsValue::null());
    };
    let fmt = text(&arg(args, 0), ctx)?;
    let (months, months_short, days, days_short) = names();
    let typed = english_names(&typed, &fmt, [&months, &months_short, &days, &days_short]);
    let date = JsDate::new(ctx);
    let this_year = date.get_full_year(ctx)?.to_number(ctx)?;
    let this_year = if this_year.is_finite() { this_year as i32 } else { 2000 };
    match scan_date(&fmt, &typed, this_year) {
        None => return Ok(JsValue::null()),
        Some(Scanned::Today) => {
            date.set_hours(&[12.into(), 0.into(), 0.into(), 0.into()], ctx)?;
        }
        Some(Scanned::At(y, mo, d, h, mi, sec)) => {
            // setFullYear, not the constructor, so years 0–99 stay as they are.
            date.set_full_year(&[y.into(), (mo.saturating_sub(1)).into(), d.into()], ctx)?;
            date.set_hours(&[h.into(), mi.into(), sec.into(), 0.into()], ctx)?;
        }
    }
    Ok(date.into())
}

/// `util.printx(cFormat, cSource)`: `?` any character, `X` letter or digit, `A` letter, `9`
/// digit, `*` the rest, `\` escape, `>` / `<` / `=` upper / lower / as is.
pub(crate) fn printx(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let fmt: Vec<char> = text(&arg(args, 0), ctx)?.chars().collect();
    let src: Vec<char> = text(&arg(args, 1), ctx)?.chars().collect();
    let (mut out, mut j, mut case) = (String::new(), 0, 0);
    let mut i = 0;
    let push = |out: &mut String, c: char, case: i32| match case {
        1 => out.extend(c.to_uppercase()),
        -1 => out.extend(c.to_lowercase()),
        _ => out.push(c),
    };
    while i < fmt.len() {
        let f = fmt[i];
        i += 1;
        let take = |j: &mut usize, ok: &dyn Fn(char) -> bool| -> Option<char> {
            while *j < src.len() {
                let c = src[*j];
                *j += 1;
                if ok(c) {
                    return Some(c);
                }
            }
            None
        };
        match f {
            '?' => {
                if let Some(c) = take(&mut j, &|_| true) {
                    push(&mut out, c, case)
                }
            }
            'X' => {
                if let Some(c) = take(&mut j, &|c| c.is_alphanumeric()) {
                    push(&mut out, c, case)
                }
            }
            'A' => {
                if let Some(c) = take(&mut j, &|c| c.is_alphabetic()) {
                    push(&mut out, c, case)
                }
            }
            '9' => {
                if let Some(c) = take(&mut j, &|c| c.is_ascii_digit()) {
                    push(&mut out, c, case)
                }
            }
            '*' => {
                for c in src[j.min(src.len())..].iter().copied() {
                    push(&mut out, c, case);
                }
                j = src.len();
            }
            '\\' if i < fmt.len() => {
                out.push(fmt[i]);
                i += 1;
            }
            '>' => case = 1,
            '<' => case = -1,
            '=' => case = 0,
            c => out.push(c),
        }
    }
    Ok(s(&out))
}

// ── stack safety ────────────────────────────────────────────────────────────────────────────
//
// boa's parser and compiler recurse once per nesting level and have no limit of their own: about
// a hundred nested parentheses overflow a 2 MiB stack, which aborts the whole app. Scripts come
// from documents, so `run` refuses nesting no real form needs and runs the engine on a thread
// with a large stack (address space, committed only as it is used). Together these bound the
// stack a script can use: brackets and prefix operators by the limits below, and every other
// construct (a few KiB per level, several bytes of source each) by the length limit.

/// Bytes `util.printd` has scanned for runs of a format character, for tests that bound its work.
#[cfg(test)]
pub(crate) static PRINTD_SCANNED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
