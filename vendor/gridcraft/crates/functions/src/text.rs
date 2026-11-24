//! Text functions.

use std::collections::HashMap;

use gridcraft_core::parse::parse_number_text_in;
use gridcraft_core::{CellError, Value, number_to_text};

use crate::criteria::wildcard_find;
use crate::util::{
    A, MAX_CELLS, MAX_TEXT, R, S, arg, array_val, as_array, boolean, has, int, num, num_val, opt_bool, opt_int, opt_num, round_half_away, scalar,
    text, text_val,
};
use crate::{Arg, Ctx, FnSpec, VAR};

const TJ: &[bool] = &[false, true, false];
const TB: &[bool] = &[true, false, true, true, true, false];
const TS: &[bool] = &[true, false, false, true, true, false];
const ATT: &[bool] = &[false, true];

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

fn check_len(s: String) -> R<Value> {
    if s.len() > MAX_TEXT && s.chars().count() > MAX_TEXT {
        return Err(CellError::Value);
    }
    text_val(s)
}

/// Pushes `piece` onto `out`, failing once the result exceeds the cell limit.
fn push_capped(out: &mut String, piece: &str) -> R<()> {
    // Checked before copying, so one oversized piece is refused without being appended.
    if out.len().saturating_add(piece.len()) > MAX_TEXT * 4 {
        return Err(CellError::Value);
    }
    out.push_str(piece);
    Ok(())
}

/// `v.to_text()` without copying text values.
fn piece(v: &Value) -> R<std::borrow::Cow<'_, str>> {
    match v {
        Value::Text(t) => Ok(std::borrow::Cow::Borrowed(t)),
        Value::Array(a) => a.data.first().map_or(Ok(std::borrow::Cow::Borrowed("")), piece),
        _ => v.to_text().map(std::borrow::Cow::Owned),
    }
}

fn concatenate(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let mut out = String::new();
    for x in a {
        push_capped(&mut out, &piece(&scalar(x))?)?;
    }
    check_len(out)
}

fn concat(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let mut out = String::new();
    for x in a {
        for v in as_array(&x.value).iter() {
            push_capped(&mut out, &piece(v)?)?;
        }
    }
    check_len(out)
}

/// The error `v.to_text()` would give, found without building any text.
fn text_error(v: &Value) -> Option<CellError> {
    match v {
        Value::Error(e) => Some(*e),
        Value::Array(a) => a.data.first().and_then(text_error),
        _ => None,
    }
}

fn textjoin(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    // Errors are found first, in argument order, so they win as before; text is then built one
    // piece at a time into the capped output, so many references to one long text (or a long
    // delimiter list) stop at the cap instead of first copying every piece.
    let delims = as_array(&arg(a, 0)?.value);
    if let Some(e) = delims.iter().find_map(text_error) {
        return Err(e);
    }
    let ignore = boolean(a, 1)?;
    let items: Vec<_> = a.iter().skip(2).map(|x| as_array(&x.value)).collect();
    if let Some(e) = items.iter().flat_map(|x| x.iter()).find_map(text_error) {
        return Err(e);
    }
    let mut out = String::new();
    let mut joined = 0usize;
    for v in items.iter().flat_map(|x| x.iter()) {
        let s = piece(v)?;
        if ignore && s.is_empty() {
            continue;
        }
        if joined > 0
            && !delims.data.is_empty()
            && let Some(d) = delims.data.get((joined - 1) % delims.data.len())
        {
            push_capped(&mut out, &piece(d)?)?;
        }
        push_capped(&mut out, &s)?;
        joined += 1;
    }
    check_len(out)
}

fn count_arg(a: &[Arg], i: usize, default: f64) -> R<usize> {
    let n = opt_num(a, i, default)?;
    if n < 0.0 || n.is_nan() {
        return Err(CellError::Value);
    }
    Ok(if n > 1e9 { 1_000_000_000 } else { n.trunc() as usize })
}

fn left(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let n = count_arg(a, 1, 1.0)?;
    text_val(s.chars().take(n).collect::<String>())
}

fn right(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = chars(&text(a, 0)?);
    let n = count_arg(a, 1, 1.0)?.min(s.len());
    text_val(s[s.len() - n..].iter().collect::<String>())
}

fn mid(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let start = num(a, 1)?;
    if start.is_nan() || start < 1.0 {
        return Err(CellError::Value);
    }
    let n = count_arg(a, 2, 0.0)?;
    let start = if start > 1e9 { 1_000_000_000 } else { start as usize };
    text_val(s.chars().skip(start - 1).take(n).collect::<String>())
}

fn len(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(text(a, 0)?.chars().count() as f64)
}

fn lower_char(ch: char) -> char {
    match ch.to_lowercase().next() {
        Some(lower) => lower,
        None => ch,
    }
}

fn upper_char(ch: char) -> char {
    let mut uppercase = ch.to_uppercase();
    match uppercase.next() {
        Some(upper) if uppercase.next().is_none() => upper,
        _ => ch,
    }
}

fn proper_upper_char(ch: char) -> char {
    if ch == '\u{130}' { 'I' } else { upper_char(ch) }
}

fn lower(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    text_val(text(a, 0)?.chars().map(lower_char).collect::<String>())
}

fn upper(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    text_val(text(a, 0)?.chars().map(upper_char).collect::<String>())
}

fn proper(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let mut out = String::with_capacity(s.len());
    let mut prev_letter = false;
    for ch in s.chars() {
        if ch.is_alphabetic() {
            if prev_letter {
                out.push(lower_char(ch));
            } else {
                out.push(proper_upper_char(ch));
            }
            prev_letter = true;
        } else {
            out.push(ch);
            prev_letter = false;
        }
    }
    text_val(out)
}

fn trim(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    text_val(s.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "))
}

fn clean(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    text_val(text(a, 0)?.chars().filter(|c| (*c as u32) >= 32).collect::<String>())
}

fn substitute(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let old = text(a, 1)?;
    let new = text(a, 2)?;
    if old.is_empty() {
        return text_val(s);
    }
    if has(a, 3) {
        let inst = num(a, 3)?;
        if inst.is_nan() || inst < 1.0 {
            return Err(CellError::Value);
        }
        let inst = inst.trunc() as usize;
        match s.match_indices(old.as_str()).nth(inst - 1) {
            Some((pos, _)) => {
                let mut out = String::with_capacity(s.len() + new.len());
                out.push_str(&s[..pos]);
                out.push_str(&new);
                out.push_str(&s[pos + old.len()..]);
                check_len(out)
            }
            None => text_val(s),
        }
    } else {
        let count = s.matches(old.as_str()).count();
        if new.len() > old.len() && s.len() + count.saturating_mul(new.len() - old.len()) > MAX_TEXT * 4 {
            return Err(CellError::Value);
        }
        check_len(s.replace(old.as_str(), &new))
    }
}

fn replace(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = chars(&text(a, 0)?);
    let start = num(a, 1)?;
    if start.is_nan() || start < 1.0 {
        return Err(CellError::Value);
    }
    let n = count_arg(a, 2, 0.0)?;
    let new = text(a, 3)?;
    let start = (if start > 1e9 { 1_000_000_000 } else { start as usize } - 1).min(s.len());
    let end = start.saturating_add(n).min(s.len());
    let mut out: String = s[..start].iter().collect();
    out.push_str(&new);
    out.extend(s[end..].iter());
    check_len(out)
}

fn rept(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let n = num(a, 1)?;
    if n.is_nan() || n < 0.0 {
        return Err(CellError::Value);
    }
    let n = n.trunc();
    let total = s.chars().count() as f64 * n;
    if total > MAX_TEXT as f64 {
        return Err(CellError::Value);
    }
    text_val(s.repeat(n as usize))
}

fn find_impl(a: &[Arg], wild: bool) -> R<Value> {
    let needle = text(a, 0)?;
    let hay = text(a, 1)?;
    let start = opt_num(a, 2, 1.0)?;
    let hc = chars(&hay);
    if (start.is_nan() || start < 1.0) || start > hc.len().max(1) as f64 {
        return Err(CellError::Value);
    }
    let start = start as usize - 1;
    if needle.is_empty() {
        return num_val((start + 1) as f64);
    }
    let pos = if wild {
        wildcard_find(&needle, &hay, start)
    } else {
        let nc = chars(&needle);
        (start..hc.len()).find(|&i| hc.get(i..i + nc.len()).is_some_and(|w| w == nc.as_slice()))
    };
    match pos {
        Some(p) => num_val((p + 1) as f64),
        None => Err(CellError::Value),
    }
}

fn find(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    find_impl(a, false)
}

fn search(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    find_impl(a, true)
}

fn exact(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    Ok(Value::Bool(text(a, 0)? == text(a, 1)?))
}

/// Windows-1252 code points 128–159.
const CP1252: [u32; 32] = [
    0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019,
    0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
];

fn char_fn(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let n = num(a, 0)?.trunc();
    if !(1.0..=255.0).contains(&n) {
        return Err(CellError::Value);
    }
    let n = n as u32;
    let cp = if (128..160).contains(&n) { CP1252[(n - 128) as usize] } else { n };
    text_val(char::from_u32(cp).map(String::from).unwrap_or_default())
}

fn code(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let ch = s.chars().next().ok_or(CellError::Value)?;
    let cp = ch as u32;
    let code = if cp < 128 || (160..256).contains(&cp) {
        cp
    } else if let Some(i) = CP1252.iter().position(|&x| x == cp) {
        128 + i as u32
    } else {
        63
    };
    num_val(code as f64)
}

fn unichar(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let n = num(a, 0)?.trunc();
    if !(1.0..=1_114_111.0).contains(&n) {
        return Err(CellError::Value);
    }
    let cp = n as u32;
    if (0xFDD0..=0xFDEF).contains(&cp) || cp & 0xFFFE == 0xFFFE {
        return Err(CellError::NA);
    }
    match char::from_u32(cp) {
        Some(c) => text_val(c.to_string()),
        None => Err(CellError::NA),
    }
}

fn unicode(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let ch = s.chars().next().ok_or(CellError::Value)?;
    num_val(ch as u32 as f64)
}

/// VALUE: a blank cell reads as 0, but empty or blank text is not a number (observed:
/// `VALUE("")` is `#VALUE!`).
fn value(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    match scalar(arg(a, 0)?) {
        Value::Number(n) => num_val(n),
        Value::Empty => num_val(0.0),
        Value::Error(e) => Err(e),
        Value::Text(t) => parse_number_text_in(&t, c.date_system()).ok_or(CellError::Value).and_then(num_val),
        _ => Err(CellError::Value),
    }
}

fn numbervalue(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let dec = if a.len() > 1 { text(a, 1)?.chars().next().ok_or(CellError::Value)? } else { '.' };
    let grp = if a.len() > 2 { text(a, 2)?.chars().next().ok_or(CellError::Value)? } else { ',' };
    if dec == grp {
        return Err(CellError::Value);
    }
    let mut body: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if body.is_empty() {
        return num_val(0.0);
    }
    // Count the trailing percent signs and cut them off in place: copying the rest once per sign
    // made a long run of them quadratic.
    let kept = body.trim_end_matches('%').len();
    let pct = body.len() - kept;
    body.truncate(kept);
    let mut seen_dec = false;
    let mut out = String::new();
    for c in body.chars() {
        if c == dec {
            if seen_dec {
                return Err(CellError::Value);
            }
            seen_dec = true;
            out.push('.');
        } else if c == grp {
            if seen_dec {
                return Err(CellError::Value);
            }
        } else if c == '.' || c == ',' {
            return Err(CellError::Value);
        } else {
            out.push(c);
        }
    }
    let mut n = parse_number_text_in(&out, c.date_system()).ok_or(CellError::Value)?;
    for _ in 0..pct {
        n /= 100.0;
    }
    num_val(n)
}

/// Formats `x` (already rounded) with `decimals` places and optional thousands separators.
fn format_fixed(x: f64, decimals: i64, commas: bool) -> String {
    let d = decimals.max(0) as usize;
    let s = x.abs().to_string();
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s.as_str(), ""),
    };
    let int_part = if commas {
        let b = int_part.as_bytes();
        let mut o = String::new();
        for (i, ch) in b.iter().enumerate() {
            if i > 0 && (b.len() - i) % 3 == 0 {
                o.push(',');
            }
            o.push(char::from(*ch));
        }
        o
    } else {
        int_part.to_string()
    };
    let mut out = int_part;
    if d > 0 {
        let frac_len = frac.len().min(d);
        out.push('.');
        out.push_str(&frac[..frac_len]);
        out.extend(std::iter::repeat_n('0', d - frac_len));
    }
    out
}

fn rounded(a: &[Arg], default_dec: f64) -> R<(f64, i64)> {
    let x = num(a, 0)?;
    let d = opt_num(a, 1, default_dec)?.trunc();
    if d > 127.0 {
        return Err(CellError::Value);
    }
    let d = d.max(-308.0) as i64;
    let r = round_half_away(x, d as i32);
    if !r.is_finite() || r.abs() >= 1e300 {
        return Err(CellError::Num);
    }
    Ok((r, d))
}

fn fixed(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (r, d) = rounded(a, 2.0)?;
    let no_commas = opt_bool(a, 2, false)?;
    let body = format_fixed(r, d, !no_commas);
    if body.len() + if r < 0.0 { 1 } else { 0 } > 255 {
        return Err(CellError::Value);
    }
    text_val(if r < 0.0 { format!("-{body}") } else { body })
}

fn dollar(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (r, d) = rounded(a, 2.0)?;
    let negative = r < 0.0 || (r == 0.0 && d >= 0 && num(a, 0)? < 0.0);
    let body = format_fixed(r, d, true);
    let out = if negative { format!("(${body})") } else { format!("${body}") };
    if out.len() > 254 {
        return Err(CellError::Value);
    }
    text_val(out)
}

// ---------------------------------------------------------------------------------------------
// BAHTTEXT

const THAI_DIGITS: [&str; 10] = ["ศูนย์", "หนึ่ง", "สอง", "สาม", "สี่", "ห้า", "หก", "เจ็ด", "แปด", "เก้า"];
/// Units, tens, hundreds, thousands, ten thousands, hundred thousands.
const THAI_PLACES: [&str; 6] = ["", "สิบ", "ร้อย", "พัน", "หมื่น", "แสน"];

/// Thai words for a group of up to six decimal digits (most significant first); empty for zero.
/// One in the tens is สิบ, two is ยี่สิบ, and a final one after higher digits is เอ็ด; `after`
/// says the number already has a non-zero digit in a higher group (so 1,000,001 ends in เอ็ด).
fn thai_group(digits: &[u8], after: bool) -> String {
    let n = digits.len();
    let higher = after || digits.iter().take(n.saturating_sub(1)).any(|d| *d != 0);
    let mut out = String::new();
    for (i, &d) in digits.iter().enumerate() {
        let place = n - 1 - i;
        match (place, d) {
            (_, 0) => {}
            (0, 1) if higher => out.push_str("เอ็ด"),
            (1, 1) => out.push_str("สิบ"),
            (1, 2) => out.push_str("ยี่สิบ"),
            _ => {
                out.push_str(THAI_DIGITS.get(d as usize).copied().unwrap_or_default());
                out.push_str(THAI_PLACES.get(place).copied().unwrap_or_default());
            }
        }
    }
    out
}

/// Thai words for a whole number written in decimal digits: groups of six digits, each but the
/// last followed by ล้าน (so 10^12 is หนึ่งล้านล้าน).
fn thai_number(digits: &str) -> String {
    let d: Vec<u8> = digits.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').skip_while(|x| *x == 0).collect();
    if d.is_empty() {
        return "ศูนย์".to_string();
    }
    let first = match d.len() % 6 {
        0 => 6,
        r => r,
    };
    let mut out = String::new();
    let mut start = 0;
    let mut len = first;
    while let Some(group) = d.get(start..start + len) {
        out.push_str(&thai_group(group, start > 0));
        start += len;
        len = 6;
        if start < d.len() {
            out.push_str("ล้าน");
        }
    }
    out
}

/// BAHTTEXT(number): the amount in Thai words with บาท, rounded to satang (สตางค์); a whole
/// number of baht ends in ถ้วน and a negative amount starts with ลบ.
fn bahttext(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = round_half_away(num(a, 0)?, 2);
    if !x.is_finite() {
        return Err(CellError::Num);
    }
    let fixed = format!("{:.2}", x.abs());
    let (whole, frac) = fixed.split_once('.').unwrap_or((fixed.as_str(), "00"));
    let baht = whole.bytes().any(|b| b != b'0');
    let satang = frac.bytes().any(|b| b != b'0');
    let mut out = String::new();
    if x < 0.0 && (baht || satang) {
        out.push_str("ลบ");
    }
    if baht || !satang {
        out.push_str(&thai_number(whole));
        out.push_str("บาท");
    }
    if satang {
        out.push_str(&thai_number(frac));
        out.push_str("สตางค์");
    } else {
        out.push_str("ถ้วน");
    }
    text_val(out)
}

fn t_fn(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    match scalar(arg(a, 0)?) {
        Value::Text(t) => Ok(Value::Text(t)),
        Value::Error(e) => Err(e),
        _ => text_val(""),
    }
}

fn identity(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    text_val(text(a, 0)?)
}

/// Lower-cased char sequence with one char per input char (keeps positions aligned).
fn fold(c: &[char]) -> Vec<char> {
    c.iter().map(|ch| ch.to_lowercase().next().unwrap_or(*ch)).collect()
}

/// Delimiter list from a scalar or array argument.
fn delimiters(v: &Value) -> R<Vec<Vec<char>>> {
    as_array(v).iter().map(|d| d.to_text().map(|s| chars(&s))).collect()
}

/// Most delimiter characters compared in one search: text length × delimiters is the formula's
/// to choose, so the search gives up with `#VALUE!` rather than run on.
const MAX_MATCH_WORK: u64 = 100_000_000;

/// Non-overlapping delimiter matches (start, end) in char positions, in ascending order. Scans left
/// to right preferring the longest delimiter starting at each position, or (`from_end`) right to
/// left preferring the longest delimiter ending at each position. Empty delimiters are ignored unless all
/// delimiters are empty, in which case every position matches with zero length. Repeated
/// delimiters are tried once, and only those whose first (or, from the end, last) character is the
/// one at hand; a search past [`MAX_MATCH_WORK`] is `#VALUE!`.
fn find_matches(text: &[char], delims: &[Vec<char>], insensitive: bool, from_end: bool) -> R<Vec<(usize, usize)>> {
    let t = if insensitive { fold(text) } else { text.to_vec() };
    let mut ds: Vec<Vec<char>> = delims.iter().filter(|d| !d.is_empty()).map(|d| if insensitive { fold(d) } else { d.clone() }).collect();
    if ds.is_empty() {
        return Ok((0..=t.len()).map(|i| (i, i)).collect());
    }
    ds.sort_unstable();
    ds.dedup();
    // Delimiters by the character a match is anchored on.
    let mut by_char: HashMap<char, Vec<&[char]>> = HashMap::new();
    for d in &ds {
        let anchor = if from_end { d.last() } else { d.first() };
        if let Some(&c) = anchor {
            by_char.entry(c).or_default().push(d);
        }
    }
    let mut work = 0u64;
    let mut longest = |candidates: Option<&Vec<&[char]>>, fits: &dyn Fn(&[char]) -> bool| -> R<Option<usize>> {
        let mut best = None;
        for d in candidates.into_iter().flatten() {
            work = work.saturating_add(d.len() as u64);
            if work > MAX_MATCH_WORK {
                return Err(CellError::Value);
            }
            if fits(d) {
                best = best.max(Some(d.len()));
            }
        }
        Ok(best)
    };
    let mut out = Vec::new();
    if from_end {
        let mut j = t.len();
        while j > 0 {
            let at = t.get(j - 1).and_then(|c| by_char.get(c));
            match longest(at, &|d| j.checked_sub(d.len()).and_then(|st| t.get(st..j)).is_some_and(|w| w == d))? {
                Some(l) => {
                    out.push((j - l, j));
                    j -= l;
                }
                None => j -= 1,
            }
        }
        out.reverse();
        return Ok(out);
    }
    let mut i = 0;
    while i < t.len() {
        let at = t.get(i).and_then(|c| by_char.get(c));
        match longest(at, &|d| t.get(i..i + d.len()).is_some_and(|w| w == d))? {
            Some(l) => {
                out.push((i, i + l));
                i += l;
            }
            None => i += 1,
        }
    }
    Ok(out)
}

fn text_before_after(a: &[Arg], before: bool) -> R<Value> {
    let s = chars(&text(a, 0)?);
    let delims = delimiters(&arg(a, 1)?.value)?;
    let inst = if has(a, 2) { int(a, 2)? } else { 1 };
    let mode = opt_int(a, 3, 0)?;
    let match_end = opt_bool(a, 4, false)?;
    if !(0..=1).contains(&mode) || inst == 0 || inst.unsigned_abs() as usize > s.len().max(1) {
        return Err(CellError::Value);
    }
    let mut m = find_matches(&s, &delims, mode == 1, inst < 0)?;
    if match_end {
        if inst > 0 {
            if m.last().is_none_or(|x| x.0 != s.len()) {
                m.push((s.len(), s.len()));
            }
        } else if m.first().is_none_or(|x| x.0 != 0) {
            m.insert(0, (0, 0));
        }
    }
    let k = inst.unsigned_abs() as usize;
    let hit = if inst > 0 { m.get(k - 1) } else { m.len().checked_sub(k).and_then(|i| m.get(i)) };
    match hit {
        Some(&(st, en)) => text_val(if before { s[..st].iter().collect::<String>() } else { s[en..].iter().collect::<String>() }),
        None => match a.get(5) {
            Some(x) => Ok(scalar(x)),
            None => Err(CellError::NA),
        },
    }
}

fn textbefore(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    text_before_after(a, true)
}

fn textafter(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    text_before_after(a, false)
}

fn split_by(s: &[char], delims: &[Vec<char>], insensitive: bool) -> R<Vec<Vec<char>>> {
    if delims.iter().all(Vec::is_empty) {
        return Ok(vec![s.to_vec()]);
    }
    let mut out = Vec::new();
    let mut last = 0;
    for (st, en) in find_matches(s, delims, insensitive, false)? {
        out.push(s[last..st].to_vec());
        last = en;
    }
    out.push(s[last..].to_vec());
    Ok(out)
}

fn textsplit(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = chars(&text(a, 0)?);
    let col_d = if has(a, 1) { delimiters(&arg(a, 1)?.value)? } else { vec![] };
    let row_d = if has(a, 2) { delimiters(&arg(a, 2)?.value)? } else { vec![] };
    if col_d.iter().all(|d| d.is_empty()) && row_d.iter().all(|d| d.is_empty()) {
        return Err(CellError::Value);
    }
    let ignore = opt_bool(a, 3, false)?;
    let mode = opt_int(a, 4, 0)?;
    if !(0..=1).contains(&mode) {
        return Err(CellError::Value);
    }
    let pad = if has(a, 5) { scalar(arg(a, 5)?) } else { Value::Error(CellError::NA) };
    let mut rows: Vec<Vec<String>> = Vec::new();
    for r in split_by(&s, &row_d, mode == 1)? {
        if ignore && r.is_empty() {
            continue;
        }
        let cells: Vec<String> =
            split_by(&r, &col_d, mode == 1)?.into_iter().filter(|c| !(ignore && c.is_empty())).map(|c| c.into_iter().collect()).collect();
        if ignore && cells.is_empty() {
            continue;
        }
        rows.push(cells);
    }
    if rows.is_empty() {
        return Err(CellError::Calc);
    }
    let ncols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let nrows = rows.len();
    if nrows.saturating_mul(ncols) > MAX_CELLS {
        return Err(CellError::Num);
    }
    let mut data = Vec::with_capacity(nrows * ncols);
    for r in rows {
        let n = r.len();
        data.extend(r.into_iter().map(Value::from));
        data.extend(std::iter::repeat_n(pad.clone(), ncols - n));
    }
    array_val(nrows, ncols, data)
}

fn value_text(v: &Value, strict: bool) -> String {
    match v {
        Value::Text(t) if strict => format!("\"{}\"", t.replace('"', "\"\"")),
        Value::Text(t) => t.to_string(),
        Value::Error(e) => e.as_str().to_string(),
        Value::Number(n) => number_to_text(*n),
        other => other.to_text().unwrap_or_default(),
    }
}

fn format_arg(a: &[Arg], i: usize) -> R<bool> {
    match opt_int(a, i, 0)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(CellError::Value),
    }
}

fn valuetotext(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let strict = format_arg(a, 1)?;
    check_len(value_text(&scalar(arg(a, 0)?), strict))
}

fn arraytotext(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let strict = format_arg(a, 1)?;
    let arr = as_array(&arg(a, 0)?.value);
    let mut out = String::new();
    if strict {
        out.push('{');
    }
    for r in 0..arr.rows {
        for c in 0..arr.cols {
            if r > 0 && c == 0 {
                out.push_str(if strict { ";" } else { ", " });
            } else if c > 0 {
                out.push_str(if strict { "," } else { ", " });
            }
            push_capped(&mut out, &value_text(arr.get(r, c).unwrap_or(&Value::Empty), strict))?;
        }
    }
    if strict {
        out.push('}');
    }
    check_len(out)
}

fn build_regex(pattern: &str, insensitive: bool) -> R<regex::Regex> {
    if pattern.len() > MAX_TEXT {
        return Err(CellError::Value);
    }
    regex::RegexBuilder::new(pattern)
        .case_insensitive(insensitive)
        .size_limit(1 << 22)
        .dfa_size_limit(1 << 22)
        .nest_limit(100)
        .build()
        .map_err(|_| CellError::Value)
}

fn case_arg(a: &[Arg], i: usize) -> R<bool> {
    match opt_int(a, i, 0)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(CellError::Value),
    }
}

fn regextest(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let re = build_regex(&text(a, 1)?, case_arg(a, 2)?)?;
    Ok(Value::Bool(re.is_match(&s)))
}

fn regexextract(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let mode = opt_int(a, 2, 0)?;
    let re = build_regex(&text(a, 1)?, case_arg(a, 3)?)?;
    match mode {
        0 => re.find(&s).map(|m| Value::from(m.as_str())).ok_or(CellError::NA),
        1 => {
            let all: Vec<Value> = re.find_iter(&s).take(1_000_000).map(|m| Value::from(m.as_str())).collect();
            if all.is_empty() {
                return Err(CellError::NA);
            }
            array_val(all.len(), 1, all)
        }
        2 => {
            let caps = re.captures(&s).ok_or(CellError::NA)?;
            if caps.len() <= 1 {
                return Ok(Value::from(caps.get(0).map_or("", |m| m.as_str())));
            }
            let groups: Vec<Value> = (1..caps.len()).map(|i| Value::from(caps.get(i).map_or("", |m| m.as_str()))).collect();
            array_val(1, groups.len(), groups)
        }
        _ => Err(CellError::Value),
    }
}

/// Converts Excel-style `$1` references to the regex crate's unambiguous `${1}` form.
fn convert_replacement(r: &str) -> String {
    let mut out = String::with_capacity(r.len() + 4);
    let c: Vec<char> = r.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == '$' && c.get(i + 1).is_some_and(char::is_ascii_digit) {
            let mut j = i + 1;
            while c.get(j).is_some_and(char::is_ascii_digit) {
                j += 1;
            }
            out.push_str("${");
            out.extend(c[i + 1..j].iter());
            out.push('}');
            i = j;
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

/// A piece of a replacement: literal text or a capture group (by number or name).
enum Piece {
    Text(String),
    Index(usize),
    Name(String),
}

/// A replacement split into pieces as `regex::Captures::expand` reads it: `$$` is `$`, `$name` takes
/// the longest run of `[_0-9A-Za-z]`, `${name}` ends at `}` (`${}` is empty), a name of digits is a
/// group number, and a `$` that starts no reference is itself.
fn template(rep: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut rest = rep;
    while let Some(i) = rest.find('$') {
        lit.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix('$') {
            lit.push('$');
            rest = r;
            continue;
        }
        // A braced name is a reference even when empty (it expands to nothing).
        let braced = rest.strip_prefix('{').and_then(|b| b.find('}').map(|j| (&b[..j], &b[j + 1..])));
        let (name, after) = braced.unwrap_or_else(|| {
            let j = rest.find(|c: char| !(c == '_' || c.is_ascii_alphanumeric())).unwrap_or(rest.len());
            (&rest[..j], &rest[j..])
        });
        if braced.is_none() && name.is_empty() {
            lit.push('$');
            continue;
        }
        if !lit.is_empty() {
            out.push(Piece::Text(std::mem::take(&mut lit)));
        }
        out.push(match name.parse::<usize>() {
            Ok(n) => Piece::Index(n),
            Err(_) => Piece::Name(name.to_string()),
        });
        rest = after;
    }
    lit.push_str(rest);
    if !lit.is_empty() {
        out.push(Piece::Text(lit));
    }
    out
}

/// Append a match's replacement to `out`, piece by piece; `None` once `out` would pass `limit`
/// bytes, before anything that long is built (#2162).
fn expand_capped(cap: &regex::Captures, pieces: &[Piece], out: &mut String, limit: usize) -> Option<()> {
    for p in pieces {
        let s = match p {
            Piece::Text(t) => t.as_str(),
            Piece::Index(i) => cap.get(*i).map_or("", |m| m.as_str()),
            Piece::Name(n) => cap.name(n).map_or("", |m| m.as_str()),
        };
        if out.len().saturating_add(s.len()) > limit {
            return None;
        }
        out.push_str(s);
    }
    Some(())
}

fn regexreplace(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let s = text(a, 0)?;
    let rep = template(&convert_replacement(&text(a, 2)?));
    let occ = opt_int(a, 3, 0)?;
    let re = build_regex(&text(a, 1)?, case_arg(a, 4)?)?;
    // Built match by match within a bound past which the text can only be too long (a character
    // is at most four bytes), instead of building every replacement first (#2162).
    let limit = MAX_TEXT * 4;
    let mut out = String::new();
    if occ == 0 {
        let mut last = 0;
        for cap in re.captures_iter(&s) {
            let Some(m) = cap.get(0) else { continue };
            out.push_str(s.get(last..m.start()).unwrap_or_default());
            expand_capped(&cap, &rep, &mut out, limit).ok_or(CellError::Value)?;
            last = m.end();
        }
        out.push_str(s.get(last..).unwrap_or_default());
        return check_len(out);
    }
    // An occurrence past what `usize` holds (32-bit targets) names no match: the text is returned as it was.
    let Ok(k) = usize::try_from(occ.unsigned_abs()) else { return text_val(s) };
    // Keeps only the wanted match's captures: the nth from the front, or a window of the last `k`.
    let cap = if occ > 0 {
        re.captures_iter(&s).nth(k - 1)
    } else {
        let mut last = std::collections::VecDeque::new();
        for cap in re.captures_iter(&s) {
            if last.len() == k {
                last.pop_front();
            }
            last.push_back(cap);
        }
        if last.len() == k { last.pop_front() } else { None }
    };
    let Some(cap) = cap else { return text_val(s) };
    let Some(m) = cap.get(0) else { return text_val(s) };
    out.push_str(s.get(..m.start()).unwrap_or_default());
    expand_capped(&cap, &rep, &mut out, limit).ok_or(CellError::Value)?;
    out.push_str(s.get(m.end()..).unwrap_or_default());
    check_len(out)
}

pub(crate) fn specs() -> Vec<FnSpec> {
    vec![
        f!("CONCATENATE", 1, VAR, Text, S, "CONCATENATE(text1, [text2], ...)", "Joins several text values into one.", concatenate),
        f!("CONCAT", 1, VAR, Text, A, "CONCAT(text1, [text2], ...)", "Joins text from values and every cell of ranges, without a separator.", concat),
        f!(
            "TEXTJOIN",
            3,
            VAR,
            Text,
            TJ,
            "TEXTJOIN(delimiter, ignore_empty, text1, [text2], ...)",
            "Joins text from values and ranges with a delimiter, optionally skipping empty items.",
            textjoin
        ),
        f!("LEFT", 1, 2, Text, S, "LEFT(text, [num_chars])", "Returns the first characters of a text.", left),
        f!("LEFTB", 1, 2, Text, S, "LEFTB(text, [num_bytes])", "Byte version of LEFT (same as LEFT for single-byte text).", left),
        f!("RIGHT", 1, 2, Text, S, "RIGHT(text, [num_chars])", "Returns the last characters of a text.", right),
        f!("RIGHTB", 1, 2, Text, S, "RIGHTB(text, [num_bytes])", "Byte version of RIGHT (same as RIGHT for single-byte text).", right),
        f!("MID", 3, 3, Text, S, "MID(text, start_num, num_chars)", "Returns characters from the middle of a text.", mid),
        f!("MIDB", 3, 3, Text, S, "MIDB(text, start_num, num_bytes)", "Byte version of MID (same as MID for single-byte text).", mid),
        f!("LEN", 1, 1, Text, S, "LEN(text)", "Counts the characters in a text.", len),
        f!("LENB", 1, 1, Text, S, "LENB(text)", "Byte version of LEN (same as LEN for single-byte text).", len),
        f!("LOWER", 1, 1, Text, S, "LOWER(text)", "Converts text to lower case.", lower),
        f!("UPPER", 1, 1, Text, S, "UPPER(text)", "Converts text to upper case.", upper),
        f!("PROPER", 1, 1, Text, S, "PROPER(text)", "Capitalises the first letter of each word and lower-cases the rest.", proper),
        f!("TRIM", 1, 1, Text, S, "TRIM(text)", "Removes leading and trailing spaces and collapses runs of spaces to one.", trim),
        f!("CLEAN", 1, 1, Text, S, "CLEAN(text)", "Removes non-printable control characters from text.", clean),
        f!(
            "SUBSTITUTE",
            3,
            4,
            Text,
            S,
            "SUBSTITUTE(text, old_text, new_text, [instance_num])",
            "Replaces occurrences of one text with another, optionally only a given occurrence.",
            substitute
        ),
        f!("REPLACE", 4, 4, Text, S, "REPLACE(old_text, start_num, num_chars, new_text)", "Replaces part of a text by position.", replace),
        f!("REPLACEB", 4, 4, Text, S, "REPLACEB(old_text, start_num, num_bytes, new_text)", "Byte version of REPLACE.", replace),
        f!("REPT", 2, 2, Text, S, "REPT(text, number_times)", "Repeats a text a given number of times.", rept),
        f!("FIND", 2, 3, Text, S, "FIND(find_text, within_text, [start_num])", "Finds the position of one text in another (case-sensitive).", find),
        f!("FINDB", 2, 3, Text, S, "FINDB(find_text, within_text, [start_num])", "Byte version of FIND.", find),
        f!(
            "SEARCH",
            2,
            3,
            Text,
            S,
            "SEARCH(find_text, within_text, [start_num])",
            "Finds the position of one text in another, ignoring case and allowing wildcards.",
            search
        ),
        f!("SEARCHB", 2, 3, Text, S, "SEARCHB(find_text, within_text, [start_num])", "Byte version of SEARCH.", search),
        f!("EXACT", 2, 2, Text, S, "EXACT(text1, text2)", "TRUE when two texts are identical, including case.", exact),
        f!("CHAR", 1, 1, Text, S, "CHAR(number)", "Returns the character for a code from the Windows-1252 character set.", char_fn),
        f!("CODE", 1, 1, Text, S, "CODE(text)", "Returns the Windows-1252 code of the first character of a text.", code),
        f!("UNICHAR", 1, 1, Text, S, "UNICHAR(number)", "Returns the character for a Unicode code point.", unichar),
        f!("UNICODE", 1, 1, Text, S, "UNICODE(text)", "Returns the Unicode code point of the first character of a text.", unicode),
        f!("VALUE", 1, 1, Text, S, "VALUE(text)", "Converts text that looks like a number, date or time into a number.", value),
        f!(
            "NUMBERVALUE",
            1,
            3,
            Text,
            S,
            "NUMBERVALUE(text, [decimal_separator], [group_separator])",
            "Converts text to a number using the given decimal and group separators.",
            numbervalue
        ),
        f!(
            "FIXED",
            1,
            3,
            Text,
            S,
            "FIXED(number, [decimals], [no_commas])",
            "Rounds a number and formats it as text with fixed decimals and optional thousands separators.",
            fixed
        ),
        f!("DOLLAR", 1, 2, Text, S, "DOLLAR(number, [decimals])", "Formats a number as currency text.", dollar),
        f!("USDOLLAR", 1, 2, Compatibility, S, "USDOLLAR(number, [decimals])", "Formats a number as currency text, like DOLLAR.", dollar),
        f!("BAHTTEXT", 1, 1, Text, S, "BAHTTEXT(number)", "Spells out a number in Thai words with Baht.", bahttext),
        f!("T", 1, 1, Text, S, "T(value)", "Returns the value if it is text, otherwise empty text.", t_fn),
        f!(
            "TEXTBEFORE",
            2,
            6,
            Text,
            TB,
            "TEXTBEFORE(text, delimiter, [instance_num], [match_mode], [match_end], [if_not_found])",
            "Returns the text that comes before a delimiter.",
            textbefore
        ),
        f!(
            "TEXTAFTER",
            2,
            6,
            Text,
            TB,
            "TEXTAFTER(text, delimiter, [instance_num], [match_mode], [match_end], [if_not_found])",
            "Returns the text that comes after a delimiter.",
            textafter
        ),
        f!(
            "TEXTSPLIT",
            2,
            6,
            Text,
            TS,
            "TEXTSPLIT(text, col_delimiter, [row_delimiter], [ignore_empty], [match_mode], [pad_with])",
            "Splits text into a grid of columns and rows at the given delimiters.",
            textsplit
        ),
        f!(
            "VALUETOTEXT",
            1,
            2,
            Text,
            S,
            "VALUETOTEXT(value, [format])",
            "Converts any value to text, optionally in strict (quoted) form.",
            valuetotext
        ),
        f!(
            "ARRAYTOTEXT",
            1,
            2,
            Text,
            ATT,
            "ARRAYTOTEXT(array, [format])",
            "Converts an array to text, either as a comma list or as a strict array constant.",
            arraytotext
        ),
        f!(
            "REGEXTEST",
            2,
            3,
            Text,
            S,
            "REGEXTEST(text, pattern, [case_sensitivity])",
            "TRUE when a regular expression matches somewhere in the text.",
            regextest
        ),
        f!(
            "REGEXEXTRACT",
            2,
            4,
            Text,
            S,
            "REGEXEXTRACT(text, pattern, [return_mode], [case_sensitivity])",
            "Extracts the first match, all matches or the capture groups of a regular expression.",
            regexextract
        ),
        f!(
            "REGEXREPLACE",
            3,
            5,
            Text,
            S,
            "REGEXREPLACE(text, pattern, replacement, [occurrence], [case_sensitivity])",
            "Replaces matches of a regular expression, all of them or a chosen occurrence.",
            regexreplace
        ),
        f!("ASC", 1, 1, Text, S, "ASC(text)", "Converts full-width characters to half-width (no change for single-byte text).", identity),
        f!("DBCS", 1, 1, Text, S, "DBCS(text)", "Converts half-width characters to full-width (no change in this locale).", identity),
        f!("JIS", 1, 1, Text, S, "JIS(text)", "Converts half-width characters to full-width (no change in this locale).", identity),
        f!("PHONETIC", 1, 1, Text, S, "PHONETIC(reference)", "Returns the phonetic (furigana) text; here the text itself.", identity),
    ]
}

#[cfg(test)]
mod tests {
    use gridcraft_core::{CellError, Value};

    /// The pieces expand exactly as `regex::Captures::expand` does.
    #[test]
    fn template_expands_like_regex() {
        let re = regex::Regex::new(r"(?<y>\d+)-(\d+)").unwrap();
        let cap = re.captures("x 2026-10 y").unwrap();
        for rep in ["$1", "${1}", "$1a", "${1}a", "$y", "${y}!", "$$", "$$1", "a$", "$}", "${", "${}", "${y", "$0|$2|$9", "$-1", "€$1€", ""] {
            let mut want = String::new();
            cap.expand(rep, &mut want);
            let mut got = String::new();
            assert!(super::expand_capped(&cap, &super::template(rep), &mut got, usize::MAX).is_some());
            assert_eq!(got, want, "{rep:?}");
        }
    }

    use crate::util::testutil::*;

    /// Trailing percent signs each divide by 100, and a long run of them is cut off in one pass.
    #[test]
    fn numbervalue_percent_signs() {
        close(ev("NUMBERVALUE", vec![t("5%%")]), 0.0005);
        close(ev("NUMBERVALUE", vec![t("1,5%"), t(","), t(".")]), 0.015);
        let many = format!("1{}", "%".repeat(32_000));
        close(ev("NUMBERVALUE", vec![t(&many)]), 0.0);
    }

    #[test]
    fn bahttext() {
        let bt = |x: f64| ev("BAHTTEXT", vec![n(x)]);
        // Microsoft's example.
        is_text(bt(1234.0), "หนึ่งพันสองร้อยสามสิบสี่บาทถ้วน");
        // Tens, เอ็ด, satang, millions and beyond, sign and rounding to satang.
        is_text(bt(0.0), "ศูนย์บาทถ้วน");
        is_text(bt(21.25), "ยี่สิบเอ็ดบาทยี่สิบห้าสตางค์");
        is_text(bt(0.5), "ห้าสิบสตางค์");
        is_text(bt(101.0), "หนึ่งร้อยเอ็ดบาทถ้วน");
        is_text(bt(1_000_000.0), "หนึ่งล้านบาทถ้วน");
        is_text(bt(11_000_000.0), "สิบเอ็ดล้านบาทถ้วน");
        is_text(bt(2_000_005_000_000.0), "สองล้านห้าล้านบาทถ้วน");
        is_text(bt(1e12), "หนึ่งล้านล้านบาทถ้วน");
        is_text(bt(-15.5), "ลบสิบห้าบาทห้าสิบสตางค์");
        is_text(bt(1.999), "สองบาทถ้วน");
        is_text(bt(0.005), "หนึ่งสตางค์");
        is_text(bt(-0.001), "ศูนย์บาทถ้วน");
        // A final one is เอ็ด after higher digits in any group, millions included (#1234).
        is_text(bt(21_000_001.0), "ยี่สิบเอ็ดล้านเอ็ดบาทถ้วน");
        is_text(bt(1_000_001.0), "หนึ่งล้านเอ็ดบาทถ้วน");
        is_text(bt(1_000_001_000_000.0), "หนึ่งล้านเอ็ดล้านบาทถ้วน");
        is_text(bt(1001.0), "หนึ่งพันเอ็ดบาทถ้วน");
        is_text(bt(11.0), "สิบเอ็ดบาทถ้วน");
        is_text(bt(1.0), "หนึ่งบาทถ้วน");
        is_text(bt(0.01), "หนึ่งสตางค์");
        is_text(bt(0.21), "ยี่สิบเอ็ดสตางค์");
        is_text(bt(1250.5), "หนึ่งพันสองร้อยห้าสิบบาทห้าสิบสตางค์");
        is_text(bt(0.25), "ยี่สิบห้าสตางค์");
        is_err(ev("BAHTTEXT", vec![t("abc")]), CellError::Value);
        // Huge numbers give text (or #NUM! where rounding overflows), never a panic.
        assert!(matches!(bt(-1e300), Value::Text(_)));
        assert!(matches!(bt(f64::MAX), Value::Text(_) | Value::Error(CellError::Num)));
    }

    #[test]
    fn concat_family() {
        is_text(ev("CONCATENATE", vec![t("a"), n(1.5), b(true)]), "a1.5TRUE");
        is_text(ev("CONCAT", vec![av(arr(vec![vec![tv("a"), tv("b")], vec![nv(1.0), Value::Empty]])), t("!")]), "ab1!");
        is_text(ev("TEXTJOIN", vec![t("-"), b(true), av(arr(vec![vec![tv("a"), tv(""), tv("b")]])), t("c")]), "a-b-c");
        is_text(ev("TEXTJOIN", vec![t("-"), b(false), av(arr(vec![vec![tv("a"), tv(""), tv("b")]]))]), "a--b");
        is_text(ev("TEXTJOIN", vec![av(row_t(&[",", ";"])), b(true), t("a"), t("b"), t("c")]), "a,b;c");
        is_err(ev("CONCAT", vec![t("a"), e(CellError::NA)]), CellError::NA);
        // Lifting: CONCATENATE over an array.
        assert_eq!(rows_of(&ev("CONCATENATE", vec![av(row_t(&["a", "b"])), t("x")])), vec![vec![tv("ax"), tv("bx")]]);
    }

    /// Security: TEXTJOIN builds its result piece by piece and stops at the text cap, rather
    /// than first copying every input (100,000 references to one 32,767-character text were
    /// 3 GB of copies). Errors among the inputs still win, in argument order.
    #[test]
    fn textjoin_stops_at_the_text_cap() {
        use gridcraft_core::Array;
        let long = tv(&"x".repeat(32_767));
        let many = || av(Array::filled(100_000, 1, long.clone()).into());
        is_err(ev("TEXTJOIN", vec![t(","), b(true), many()]), CellError::Value);
        is_err(ev("TEXTJOIN", vec![av(Array::filled(1, 100_000, long.clone()).into()), b(false), t("a"), t("b")]), CellError::Value);
        is_err(ev("TEXTJOIN", vec![t(","), b(true), many(), e(CellError::Div0)]), CellError::Div0);
        is_err(ev("TEXTJOIN", vec![av(arr(vec![vec![tv(","), Value::Error(CellError::NA)]])), b(true), t("a")]), CellError::NA);
        // Delimiters cycle and are converted only where used.
        is_text(ev("TEXTJOIN", vec![av(arr(vec![vec![nv(1.0), tv(";")]])), b(false), t("a"), t("b"), t("c"), t("d")]), "a1b;c1d");
    }

    fn row_t(xs: &[&str]) -> Value {
        arr(vec![xs.iter().map(|x| tv(x)).collect()])
    }

    #[test]
    fn slicing() {
        is_text(ev("LEFT", vec![t("Hello")]), "H");
        is_text(ev("LEFT", vec![t("Hello"), n(3.0)]), "Hel");
        is_text(ev("LEFT", vec![t("Hello"), n(99.0)]), "Hello");
        is_err(ev("LEFT", vec![t("Hello"), n(-1.0)]), CellError::Value);
        is_text(ev("RIGHT", vec![t("Hello"), n(2.0)]), "lo");
        is_text(ev("RIGHT", vec![t("héllo"), n(4.0)]), "éllo");
        is_text(ev("MID", vec![t("Hello"), n(2.0), n(3.0)]), "ell");
        is_text(ev("MID", vec![t("Hello"), n(10.0), n(3.0)]), "");
        is_err(ev("MID", vec![t("Hello"), n(0.0), n(3.0)]), CellError::Value);
        close(ev("LEN", vec![t("héllo")]), 5.0);
        close(ev("LEN", vec![n(123.45)]), 6.0);
        close(ev("LEN", vec![empty()]), 0.0);
        is_text(ev("LEFT", vec![n(12345.0), n(2.0)]), "12");
    }

    #[test]
    fn case_and_cleanup() {
        is_text(ev("UPPER", vec![t("abc")]), "ABC");
        is_text(ev("LOWER", vec![t("ABC")]), "abc");
        is_text(ev("UPPER", vec![t("Straße")]), "STRAßE");
        is_text(ev("LOWER", vec![t("İ")]), "i");
        is_text(ev("PROPER", vec![t("İSTANBUL")]), "Istanbul");
        is_text(ev("PROPER", vec![t("ß")]), "ß");
        is_text(ev("PROPER", vec![t("ÉCOLE")]), "École");
        let proper = ev("PROPER", vec![t("İSTANBUL Straße")]);
        is_text(proper.clone(), "Istanbul Straße");
        assert_eq!(
            match proper {
                Value::Text(s) => s.chars().count(),
                _ => 0,
            },
            "Istanbul Straße".chars().count()
        );
        is_text(ev("PROPER", vec![t("this is a TITLE")]), "This Is A Title");
        is_text(ev("PROPER", vec![t("2-way street's")]), "2-Way Street'S");
        is_text(ev("TRIM", vec![t("  a   b  c ")]), "a b c");
        is_text(ev("CLEAN", vec![t("a\u{7}b\nc")]), "abc");
    }

    #[test]
    fn substitute_replace_rept() {
        is_text(ev("SUBSTITUTE", vec![t("a-b-c"), t("-"), t("+")]), "a+b+c");
        is_text(ev("SUBSTITUTE", vec![t("a-b-c"), t("-"), t("+"), n(2.0)]), "a-b+c");
        is_text(ev("SUBSTITUTE", vec![t("a-b-c"), t("-"), t("+"), n(5.0)]), "a-b-c");
        is_err(ev("SUBSTITUTE", vec![t("a"), t("a"), t("b"), n(0.0)]), CellError::Value);
        is_text(ev("SUBSTITUTE", vec![t("abc"), t(""), t("x")]), "abc");
        is_text(ev("REPLACE", vec![t("abcdef"), n(2.0), n(3.0), t("XY")]), "aXYef");
        is_text(ev("REPLACE", vec![t("abc"), n(10.0), n(1.0), t("Z")]), "abcZ");
        is_text(ev("REPT", vec![t("ab"), n(3.0)]), "ababab");
        is_text(ev("REPT", vec![t("ab"), n(0.0)]), "");
        is_err(ev("REPT", vec![t("ab"), n(-1.0)]), CellError::Value);
        is_err(ev("REPT", vec![t("ab"), n(20000.0)]), CellError::Value);
        is_err(ev("REPT", vec![t("ab"), n(1e300)]), CellError::Value);
    }

    #[test]
    fn find_search() {
        close(ev("FIND", vec![t("b"), t("abcb")]), 2.0);
        close(ev("FIND", vec![t("b"), t("abcb"), n(3.0)]), 4.0);
        is_err(ev("FIND", vec![t("B"), t("abc")]), CellError::Value);
        close(ev("FIND", vec![t(""), t("abc")]), 1.0);
        is_err(ev("FIND", vec![t("a"), t("abc"), n(0.0)]), CellError::Value);
        close(ev("SEARCH", vec![t("B"), t("abc")]), 2.0);
        close(ev("SEARCH", vec![t("c*"), t("abcd")]), 3.0);
        close(ev("SEARCH", vec![t("?d"), t("abcd")]), 3.0);
        close(ev("SEARCH", vec![t("~*"), t("a*b")]), 2.0);
        is_err(ev("SEARCH", vec![t("z"), t("abc")]), CellError::Value);
        is_err(ev("SEARCH", vec![t("a"), t("abc"), n(5.0)]), CellError::Value);
        close(ev("SEARCH", vec![t("b"), t("\u{130}b")]), 2.0);
        close(ev("SEARCH", vec![t("B"), t("\u{130}b")]), 2.0);
        close(ev("SEARCH", vec![t("b?"), t("a\u{130}bc")]), 3.0);
        close(ev("SEARCH", vec![t("i"), t("a\u{130}bc")]), 2.0);
        is_err(ev("SEARCH", vec![t("b"), t("a\u{130}bc"), n(4.0)]), CellError::Value);
        for f in ["FIND", "SEARCH"] {
            close(ev(f, vec![t(""), t("abc"), n(3.0)]), 3.0);
            is_err(ev(f, vec![t(""), t("abc"), n(4.0)]), CellError::Value);
        }
        assert_eq!(ev("EXACT", vec![t("a"), t("A")]), Value::Bool(false));
        assert_eq!(ev("EXACT", vec![t("a"), t("a")]), Value::Bool(true));
    }

    #[test]
    fn codes() {
        is_text(ev("CHAR", vec![n(65.0)]), "A");
        is_text(ev("CHAR", vec![n(128.0)]), "€");
        is_err(ev("CHAR", vec![n(0.0)]), CellError::Value);
        is_err(ev("CHAR", vec![n(256.0)]), CellError::Value);
        close(ev("CODE", vec![t("A")]), 65.0);
        close(ev("CODE", vec![t("€")]), 128.0);
        is_err(ev("CODE", vec![t("")]), CellError::Value);
        is_text(ev("UNICHAR", vec![n(9731.0)]), "☃");
        for cp in [0xFDD0, 0xFDEF, 0xFFFE, 0xFFFF, 0x1FFFE, 0x1FFFF, 0x10FFFE, 0x10FFFF] {
            is_err(ev("UNICHAR", vec![n(f64::from(cp))]), CellError::NA);
        }
        for (cp, expected) in [(0xFDCF, "\u{FDCF}"), (0xFDF0, "\u{FDF0}"), (0xFFFD, "\u{FFFD}"), (0x1FFFD, "\u{1FFFD}")] {
            is_text(ev("UNICHAR", vec![n(f64::from(cp))]), expected);
        }
        close(ev("UNICODE", vec![t("☃")]), 9731.0);
        is_err(ev("UNICHAR", vec![n(0.0)]), CellError::Value);
        is_err(ev("UNICHAR", vec![n(1_114_112.0)]), CellError::Value);
        is_err(ev("UNICHAR", vec![n(55296.0)]), CellError::NA);
    }

    /// VALUE cases observed in Excel (ValueFunctionOfBlank, FormulaEvalTestData); a blank cell
    /// still reads as 0.
    #[test]
    fn value_observed() {
        let cases: Vec<(crate::Arg, Result<f64, CellError>)> = vec![
            (t(""), Err(CellError::Value)),
            (t("ABC"), Err(CellError::Value)),
            (t("0.33"), Ok(0.33)),
            (t("$1,000"), Ok(1000.0)),
            (rf(Value::Empty), Ok(0.0)),
        ];
        for (arg, want) in cases {
            match want {
                Ok(x) => close(ev("VALUE", vec![arg]), x),
                Err(err) => is_err(ev("VALUE", vec![arg]), err),
            }
        }
    }

    #[test]
    fn numbers_from_text() {
        close(ev("VALUE", vec![t("1,234.5")]), 1234.5);
        close(ev("VALUE", vec![t("50%")]), 0.5);
        close(ev("VALUE", vec![t("$10")]), 10.0);
        close(ev("VALUE", vec![t("12:00")]), 0.5);
        close(ev("VALUE", vec![t("2020-01-01")]), 43831.0);
        is_err(ev("VALUE", vec![t("abc")]), CellError::Value);
        // Accounting negatives, as Excel 365 reads them.
        close(ev("VALUE", vec![t("$(5)")]), -5.0);
        close(ev("VALUE", vec![t("$ (1,000.50)")]), -1000.5);
        close(ev("VALUE", vec![t("(5)%")]), -0.05);
        for s in ["$(5)%", "$(5%)", "$5%", "(-5)", "((5))", "$(-5)", "(1/2)"] {
            is_err(ev("VALUE", vec![t(s)]), CellError::Value);
        }
        close(ev("NUMBERVALUE", vec![t("2.500,27"), t(","), t(".")]), 2500.27);
        close(ev("NUMBERVALUE", vec![t("3.5%")]), 0.035);
        close(ev("NUMBERVALUE", vec![t(" 1 000 ")]), 1000.0);
        close(ev("NUMBERVALUE", vec![t("")]), 0.0);
        is_err(ev("NUMBERVALUE", vec![t("1.2.3")]), CellError::Value);
        is_err(ev("NUMBERVALUE", vec![t("1.5"), t(","), t(" ")]), CellError::Value);
        is_err(ev("NUMBERVALUE", vec![t("1,5"), t("."), t(" ")]), CellError::Value);
        is_text(ev("FIXED", vec![n(1234.567), n(1.0)]), "1,234.6");
        is_text(ev("FIXED", vec![n(1234.567), n(-1.0)]), "1,230");
        is_text(ev("FIXED", vec![n(-1234.567), n(1.0), b(true)]), "-1234.6");
        is_text(ev("FIXED", vec![n(44.332)]), "44.33");
        is_text(ev("FIXED", vec![n(2.675), n(2.0)]), "2.68");
        is_text(ev("DOLLAR", vec![n(1234.567)]), "$1,234.57");
        is_text(ev("DOLLAR", vec![n(-1234.567), n(1.0)]), "($1,234.6)");
        is_text(ev("DOLLAR", vec![n(0.5), n(0.0)]), "$1");
        is_text(ev("USDOLLAR", vec![n(1.0)]), "$1.00");
        is_text(ev("USDOLLAR", vec![n(-1234.567), n(1.0)]), "($1,234.6)");
        let fixed_16 = format!("1.1{}", "0".repeat(15));
        let dollar_16 = format!("$1.1{}", "0".repeat(15));
        let fixed_20 = format!("1.005{}", "0".repeat(17));
        let dollar_20 = format!("$1.005{}", "0".repeat(17));
        let fixed_127 = format!("1.1{}", "0".repeat(126));
        let dollar_127 = format!("$1.1{}", "0".repeat(126));
        is_text(ev("FIXED", vec![n(1.1), n(16.0)]), &fixed_16);
        is_text(ev("DOLLAR", vec![n(1.1), n(16.0)]), &dollar_16);
        is_text(ev("FIXED", vec![n(1.005), n(20.0)]), &fixed_20);
        is_text(ev("DOLLAR", vec![n(1.005), n(20.0)]), &dollar_20);
        is_text(ev("FIXED", vec![n(1.1), n(127.0)]), &fixed_127);
        is_text(ev("DOLLAR", vec![n(1.1), n(127.0)]), &dollar_127);
        is_text(ev("T", vec![t("x")]), "x");
        is_text(ev("T", vec![n(1.0)]), "");
    }

    #[test]
    fn fixed_and_dollar_output_limits() {
        assert!(matches!(ev("FIXED", vec![n(1e191), n(0.0)]), Value::Text(ref s) if s.len() == 255));
        is_err(ev("FIXED", vec![n(1e192), n(0.0)]), CellError::Value);
        assert!(matches!(ev("FIXED", vec![n(-1e253), n(0.0), b(true)]), Value::Text(ref s) if s.starts_with('-')));
        is_err(ev("FIXED", vec![n(-1e191), n(0.0)]), CellError::Value);
        assert!(matches!(ev("FIXED", vec![n(1e189), n(1.0)]), Value::Text(ref s) if s.len() == 255));
        is_err(ev("FIXED", vec![n(1e190), n(1.0)]), CellError::Value);

        assert!(matches!(ev("DOLLAR", vec![n(1e189), n(0.0)]), Value::Text(ref s) if s.len() == 254));
        is_err(ev("DOLLAR", vec![n(1e190), n(0.0)]), CellError::Value);
        assert!(matches!(ev("DOLLAR", vec![n(-1e188), n(0.0)]), Value::Text(ref s) if s.len() == 254));
        is_err(ev("DOLLAR", vec![n(-1e189), n(0.0)]), CellError::Value);
        assert!(matches!(ev("DOLLAR", vec![n(1.23456789e187), n(2.0)]), Value::Text(ref s) if s.len() == 254));
        is_err(ev("DOLLAR", vec![n(1.23456789e188), n(2.0)]), CellError::Value);
        is_err(ev("DOLLAR", vec![n(-1.23456789e186), n(2.0)]), CellError::Value);
        assert!(matches!(ev("DOLLAR", vec![n(1e189), n(-1.0)]), Value::Text(ref s) if s.len() == 254));
        is_err(ev("DOLLAR", vec![n(1e190), n(-1.0)]), CellError::Value);
    }

    #[test]
    fn dollar_keeps_negative_sign_when_nonnegative_precision_rounds_to_zero() {
        is_text(ev("DOLLAR", vec![n(-0.0001), n(0.0)]), "($0)");
        is_text(ev("DOLLAR", vec![n(-0.0001), n(2.0)]), "($0.00)");
        is_text(ev("DOLLAR", vec![n(0.0), n(0.0)]), "$0");
        is_text(ev("DOLLAR", vec![n(-0.0), n(0.0)]), "$0");
        is_text(ev("DOLLAR", vec![n(0.0001), n(0.0)]), "$0");
        is_text(ev("DOLLAR", vec![n(-0.0001), n(-1.0)]), "$0");
        is_text(ev("DOLLAR", vec![n(-4.0), n(-1.0)]), "$0");
        is_text(ev("DOLLAR", vec![n(-5.0), n(-1.0)]), "($10)");
        is_text(ev("DOLLAR", vec![n(-0.4), n(0.0)]), "($0)");
        is_text(ev("DOLLAR", vec![n(-0.5), n(0.0)]), "($1)");
        is_text(ev("FIXED", vec![n(-0.0001), n(0.0)]), "0");
    }

    #[test]
    fn before_after() {
        is_text(ev("TEXTBEFORE", vec![t("Red riding hood"), t(" ")]), "Red");
        is_text(ev("TEXTBEFORE", vec![t("Red riding hood"), t(" "), n(2.0)]), "Red riding");
        is_text(ev("TEXTBEFORE", vec![t("Red riding hood"), t(" "), n(-1.0)]), "Red riding");
        is_text(ev("TEXTAFTER", vec![t("Red riding hood"), t(" ")]), "riding hood");
        is_text(ev("TEXTAFTER", vec![t("Red riding hood"), t(" "), n(-1.0)]), "hood");
        is_err(ev("TEXTAFTER", vec![t("abc"), t("x")]), CellError::NA);
        is_text(ev("TEXTAFTER", vec![t("abc"), t("x"), n(1.0), n(0.0), n(0.0), t("none")]), "none");
        is_text(ev("TEXTAFTER", vec![t("aXb"), t("x"), n(1.0), n(1.0)]), "b");
        is_err(ev("TEXTAFTER", vec![t("aXb"), t("x")]), CellError::NA);
        is_text(ev("TEXTBEFORE", vec![t("abc"), t("x"), n(1.0), n(0.0), n(1.0)]), "abc");
        is_text(ev("TEXTAFTER", vec![t("a-b+c"), av(row_t(&["+", "-"]))]), "b+c");
        is_err(ev("TEXTBEFORE", vec![t("abc"), t("b"), n(0.0)]), CellError::Value);
        is_text(ev("TEXTBEFORE", vec![t("abc"), t("")]), "");
        is_text(ev("TEXTAFTER", vec![t("abc"), t("")]), "abc");
        // Negative instances search from the end, so overlapping delimiters match differently.
        is_text(ev("TEXTBEFORE", vec![t("aaa"), t("aa"), n(-1.0)]), "a");
        is_text(ev("TEXTAFTER", vec![t("aaa"), t("aa"), n(-1.0)]), "");
        is_text(ev("TEXTBEFORE", vec![t("aaaa"), t("aa"), n(-2.0)]), "");
        is_text(ev("TEXTBEFORE", vec![t("AaA"), t("aa"), n(-1.0), n(1.0)]), "A");
        is_text(ev("TEXTAFTER", vec![t("aaa"), t("aa"), n(1.0)]), "a");
    }

    /// Thousands of copies of one delimiter are tried once; delimiters that can't start where the
    /// scan stands aren't compared; a search over a budget of comparisons is `#VALUE!`.
    #[test]
    fn delimiter_matching_stays_bounded() {
        let text = "ab,".repeat(10_000);
        let same: Vec<&str> = vec![","; 50_000];
        let v = ev("TEXTSPLIT", vec![t(&text), av(row_t(&same))]);
        assert_eq!(rows_of(&v).first().map(Vec::len), Some(10_001));
        // Distinct long delimiters that start and end like every position: refused, not run.
        let long: Vec<String> = (0..5_000).map(|i| format!("a{i}{}", "a".repeat(1_000))).collect();
        let long: Vec<&str> = long.iter().map(String::as_str).collect();
        is_err(ev("TEXTSPLIT", vec![t(&"a".repeat(30_000)), av(row_t(&long))]), CellError::Value);
        is_err(ev("TEXTBEFORE", vec![t(&"a".repeat(30_000)), av(row_t(&long)), n(-1.0)]), CellError::Value);
        // Longest match wins at a position, from either end.
        is_text(ev("TEXTBEFORE", vec![t("x--y-z"), av(row_t(&["-", "--"]))]), "x");
        is_text(ev("TEXTAFTER", vec![t("x--y-z"), av(row_t(&["-", "--"])), n(-1.0)]), "z");
    }

    #[test]
    fn split() {
        let v = ev("TEXTSPLIT", vec![t("a,b;c,d"), t(","), t(";")]);
        assert_eq!(rows_of(&v), vec![vec![tv("a"), tv("b")], vec![tv("c"), tv("d")]]);
        let v = ev("TEXTSPLIT", vec![t("a,b;c"), t(","), t(";")]);
        assert_eq!(rows_of(&v), vec![vec![tv("a"), tv("b")], vec![tv("c"), Value::Error(CellError::NA)]]);
        let v = ev("TEXTSPLIT", vec![t("a,,b"), t(","), empty(), b(true)]);
        assert_eq!(rows_of(&v), vec![vec![tv("a"), tv("b")]]);
        let v = ev("TEXTSPLIT", vec![t("a,,b"), t(",")]);
        assert_eq!(rows_of(&v), vec![vec![tv("a"), tv(""), tv("b")]]);
        let v = ev("TEXTSPLIT", vec![t("a b-c"), av(row_t(&[" ", "-"]))]);
        assert_eq!(rows_of(&v), vec![vec![tv("a"), tv("b"), tv("c")]]);
        let v = ev("TEXTSPLIT", vec![t("1;2"), empty(), t(";")]);
        assert_eq!(rows_of(&v), vec![vec![tv("1")], vec![tv("2")]]);
        let v = ev("TEXTSPLIT", vec![t("aXbxc"), t("x"), empty(), b(false), n(1.0)]);
        assert_eq!(rows_of(&v), vec![vec![tv("a"), tv("b"), tv("c")]]);
        let v = ev("TEXTSPLIT", vec![t("a,b;c"), t(","), t(";"), b(false), n(0.0), t("-")]);
        assert_eq!(rows_of(&v)[1], vec![tv("c"), tv("-")]);
    }

    #[test]
    fn split_rejects_oversized_padded_grid() {
        // One row of 8000 columns followed by 8000 one-cell rows: the text is under the cell-text limit, the padded grid is not.
        let text = format!("{}{}", "a,".repeat(8000), "b;".repeat(8000));
        is_err(ev("TEXTSPLIT", vec![t(&text), t(","), t(";")]), CellError::Num);
    }

    #[test]
    fn to_text() {
        is_text(ev("VALUETOTEXT", vec![t("a\"b")]), "a\"b");
        is_text(ev("VALUETOTEXT", vec![t("ab"), n(1.0)]), "\"ab\"");
        is_text(ev("VALUETOTEXT", vec![n(1.5), n(1.0)]), "1.5");
        is_text(ev("VALUETOTEXT", vec![e(CellError::Div0)]), "#DIV/0!");
        is_err(ev("VALUETOTEXT", vec![n(1.0), n(2.0)]), CellError::Value);
        let a = av(arr(vec![vec![nv(1.0), tv("a")], vec![Value::Bool(true), Value::Error(CellError::NA)]]));
        is_text(ev("ARRAYTOTEXT", vec![a.clone()]), "1, a, TRUE, #N/A");
        is_text(ev("ARRAYTOTEXT", vec![a, n(1.0)]), "{1,\"a\";TRUE,#N/A}");
    }

    #[test]
    fn regex() {
        assert_eq!(ev("REGEXTEST", vec![t("abc123"), t("[0-9]+")]), Value::Bool(true));
        assert_eq!(ev("REGEXTEST", vec![t("ABC"), t("abc")]), Value::Bool(false));
        assert_eq!(ev("REGEXTEST", vec![t("ABC"), t("abc"), n(1.0)]), Value::Bool(true));
        is_err(ev("REGEXTEST", vec![t("a"), t("(")]), CellError::Value);
        is_text(ev("REGEXEXTRACT", vec![t("tel 555-1234 or 555-9876"), t("[0-9]{3}-[0-9]{4}")]), "555-1234");
        let v = ev("REGEXEXTRACT", vec![t("tel 555-1234 or 555-9876"), t("[0-9]{3}-[0-9]{4}"), n(1.0)]);
        assert_eq!(rows_of(&v), vec![vec![tv("555-1234")], vec![tv("555-9876")]]);
        let v = ev("REGEXEXTRACT", vec![t("John Smith"), t("(\\w+) (\\w+)"), n(2.0)]);
        assert_eq!(rows_of(&v), vec![vec![tv("John"), tv("Smith")]]);
        is_err(ev("REGEXEXTRACT", vec![t("abc"), t("z")]), CellError::NA);
        is_text(ev("REGEXREPLACE", vec![t("a1b22c"), t("[0-9]+"), t("#")]), "a#b#c");
        is_text(ev("REGEXREPLACE", vec![t("a1b22c"), t("[0-9]+"), t("#"), n(2.0)]), "a1b#c");
        is_text(ev("REGEXREPLACE", vec![t("a1b22c"), t("[0-9]+"), t("#"), n(-2.0)]), "a#b22c");
        is_text(ev("REGEXREPLACE", vec![t("John Smith"), t("(\\w+) (\\w+)"), t("$2, $1")]), "Smith, John");
        is_text(ev("REGEXREPLACE", vec![t("ab"), t("(a)"), t("$1x")]), "axb");
    }
}
