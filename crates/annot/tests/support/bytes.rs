//! Shared helpers compile into every integration-test binary of this crate, and not every
//! binary uses every one of them; `dead_code` would then fail `clippy -D warnings` for reasons
//! that have nothing to do with the test being written.
#![allow(dead_code)]

//! A deliberately strict reader over the *bytes* a save produced, independent of `pdfcraft-cos`.
//!
//! Reopening saved bytes with `Document::open` and comparing dictionaries proves that our writer
//! and our reader agree. It does not prove what is actually *in the file*: a lenient parser can
//! forgive output that another implementation refuses (an exponent in what should be a fixed
//! decimal number, an unterminated string, a key written twice). These helpers look at the bytes
//! themselves and reject anything outside the narrow grammar ISO 32000-2 §7.3 allows for the
//! object types annotations use, so an assertion here is a claim about the file rather than about
//! a round trip through our own pair of implementations.
//!
//! Scope: dictionary keys whose values are number arrays (`/Rect`, `/QuadPoints`, `/L`,
//! `/Vertices`, `/CL`, `/RD`), number arrays of arrays (`/InkList`), text strings (`/Contents`,
//! `/T`, `/NM`, `/M`) and names (`/Subtype`). That covers the entries these tests care about; it
//! is not a general PDF parser and does not try to be.

/// Every value token written for `/key` in the raw bytes, in file order.
///
/// A name-keyed dictionary entry is `/key` preceded and followed by delimiters, so searching for
/// `Rect` does not match inside `/MyRectangle`. The returned spans include the enclosing brackets
/// or string delimiters, which the decoders below strip.
pub fn value_tokens(bytes: &[u8], key: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + key.len() + 1 < bytes.len() {
        if bytes[i] != b'/' || &bytes[i + 1..i + 1 + key.len()] != key {
            i += 1;
            continue;
        }
        let after = i + 1 + key.len();
        // The key must end here: a longer name is a different key.
        if after < bytes.len() && is_name_byte(bytes[after]) {
            i += 1;
            continue;
        }
        let Some((end, token)) = capture(bytes, after) else {
            i += 1;
            continue;
        };
        // A `/Key /Other` hit is ambiguous: it may be a key whose value is a name, or the *name*
        // that is some earlier key's value (`/S /URI`). Re-scan from the next byte so a later real
        // occurrence of `/Key` is still found; an array, string or number value is unambiguous.
        let name_valued = token.starts_with(b"/");
        out.push(token);
        i = if name_valued { i + 1 } else { end };
    }
    out
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'#' | b'%')
}

/// Capture the object that starts at `at`, with bracket/paren balance respected. A literal
/// string's brackets do not count; an array containing a string does nest.
fn capture(bytes: &[u8], mut at: usize) -> Option<(usize, Vec<u8>)> {
    while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
        at += 1;
    }
    let first = *bytes.get(at)?;
    match first {
        b'[' => balanced(bytes, at, b'[', b']'),
        b'(' => literal_string(bytes, at),
        b'<' if bytes.get(at + 1) == Some(&b'<') => balanced(bytes, at, b'<', b'>'),
        // A hex string: `<...>`, closed by the first `>` that is not part of a `>>`.
        b'<' => {
            let close = bytes.get(at + 1..)?.iter().position(|&b| b == b'>')?;
            Some((at + close + 2, bytes[at..at + close + 2].to_vec()))
        }
        b'/' => {
            let mut j = at + 1;
            while bytes.get(j).is_some_and(|b| !is_delimiter(*b)) {
                j += 1;
            }
            Some((j, bytes[at..j].to_vec()))
        }
        _ => {
            let mut j = at;
            while bytes.get(j).is_some_and(|b| !is_delimiter(*b)) {
                j += 1;
            }
            Some((j, bytes[at..j].to_vec()))
        }
    }
}

fn is_delimiter(b: u8) -> bool {
    b.is_ascii_whitespace() || matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// `open` at `at`, closed by the matching `close`, skipping over literal strings so that a
/// delimiter inside `(…)` does not end the span.
fn balanced(bytes: &[u8], at: usize, open: u8, close: u8) -> Option<(usize, Vec<u8>)> {
    let mut depth = 0usize;
    let mut i = at;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => {
                let (end, _) = literal_string(bytes, i)?;
                i = end;
                continue;
            }
            b if b == open => depth += 1,
            b if b == close => {
                depth -= 1;
                if depth == 0 {
                    return Some((i + 1, bytes[at..=i].to_vec()));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn literal_string(bytes: &[u8], at: usize) -> Option<(usize, Vec<u8>)> {
    let mut i = at + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b')' => return Some((i + 1, bytes[at..=i].to_vec())),
            _ => i += 1,
        }
    }
    None
}

/// The numbers of an array token, written in fixed decimal notation (§7.3.3: no exponent form).
/// `None` when the token is not an array, holds a non-number, or uses `e`.
pub fn numbers(token: &[u8]) -> Option<Vec<f64>> {
    let inner = token.strip_prefix(b"[")?.strip_suffix(b"]")?.to_vec();
    let mut out = Vec::new();
    for word in split_words(&inner) {
        if word.first() == Some(&b'[') {
            // A nested array (`/InkList`): its numbers count as this one's, flattened.
            out.extend(numbers(&word)?);
        } else {
            out.push(fixed_decimal(&word)?);
        }
    }
    Some(out)
}

/// A single number written for `/key` (`/CA`, `/F`, a stream's `/Length`), strictly decimal.
pub fn scalars_of(bytes: &[u8], key: &str) -> Vec<f64> {
    value_tokens(bytes, key.as_bytes()).iter().filter_map(|t| fixed_decimal(t)).collect()
}

/// All numbers of an ARRAY token written for `/key`, with the strict fixed-decimal rule.
///
/// ONLY valid for keys whose value is an array (`/Rect`, `/QuadPoints`, `/L`, `/IC`,
/// `/PCPictureSize`). For a SCALAR key (`/W`, `/CA`, `/F`) every token fails `numbers` and the
/// result is an empty vector — so comparing two such calls against each other is vacuous: it
/// passes whether or not the file holds anything. Use [`scalars_of`] for scalars, and always
/// assert non-emptiness when a value is expected. A scalar misuse was found and fixed in
/// `round_trip.rs` (a `/BS /W` comparison that silently compared `[] == []`).
pub fn numbers_of(bytes: &[u8], key: &str) -> Vec<Vec<f64>> {
    value_tokens(bytes, key.as_bytes()).iter().filter_map(|t| numbers(t)).collect()
}

/// A number written as `[+-]?digits[.]?` or `[+-]?digits[.]digits`, with no exponent and no
/// stray characters. `str::parse::<f64>` alone would accept `1e7`, which PDF does not use.
fn fixed_decimal(word: &[u8]) -> Option<f64> {
    if word.is_empty() || word.iter().any(|b| *b == b'e' || *b == b'E') {
        return None;
    }
    let body = match word[0] {
        b'+' | b'-' => &word[1..],
        _ => word,
    };
    if body.is_empty() {
        return None;
    }
    let mut digits = 0;
    let mut dots = 0;
    for &b in body {
        match b {
            b'0'..=b'9' => digits += 1,
            b'.' => dots += 1,
            _ => return None,
        }
    }
    if digits == 0 || dots > 1 {
        return None;
    }
    std::str::from_utf8(word).ok()?.parse::<f64>().ok()
}

/// The decoded bytes of a string token (literal with escapes, or hex), `None` if malformed.
pub fn raw_string(token: &[u8]) -> Option<Vec<u8>> {
    match (token.first(), token.last()) {
        (Some(b'('), Some(b')')) => {
            let mut out = Vec::new();
            let body = &token[1..token.len() - 1];
            let mut i = 0;
            while i < body.len() {
                if body[i] != b'\\' {
                    out.push(body[i]);
                    i += 1;
                    continue;
                }
                let e = *body.get(i + 1)?;
                match e {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0C),
                    b'(' | b')' | b'\\' => out.push(e),
                    b'0'..=b'7' => {
                        let mut v = 0u32;
                        let mut j = i + 1;
                        let mut n = 0;
                        while j < body.len() && n < 3 && (b'0'..=b'7').contains(&body[j]) {
                            v = v * 8 + u32::from(body[j] - b'0');
                            j += 1;
                            n += 1;
                        }
                        out.push(u8::try_from(v).ok()?);
                        i = j;
                        continue;
                    }
                    b'\n' => {}
                    _ => return None,
                }
                i += 2;
            }
            Some(out)
        }
        (Some(b'<'), Some(b'>')) => {
            let hex: Vec<u8> = token[1..token.len() - 1].iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            if !hex.len().is_multiple_of(2) {
                return None;
            }
            let mut out = Vec::with_capacity(hex.len() / 2);
            for pair in hex.as_chunks::<2>().0 {
                let s = std::str::from_utf8(pair).ok()?;
                out.push(u8::from_str_radix(s, 16).ok()?);
            }
            Some(out)
        }
        _ => None,
    }
}

/// Text as the file holds it: UTF-16BE behind a BOM, PDF 2.0's UTF-8 BOM, or PDFDocEncoding —
/// approximated by Latin-1 above 0x7F. The two differ only at a handful of bytes between 0x18
/// and 0x9F, none of which these tests assert on; ASCII and the UTF-16 cases are exact.
pub fn text_of_raw(bytes: &[u8]) -> String {
    match bytes {
        [0xFE, 0xFF, rest @ ..] => String::from_utf16_lossy(&rest.as_chunks::<2>().0.iter().map(|p| u16::from_be_bytes(*p)).collect::<Vec<_>>()),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        b if !b.is_ascii() && std::str::from_utf8(b).is_ok() => String::from_utf8_lossy(b).into_owned(),
        b => b.iter().map(|&c| char::from(c)).collect(),
    }
}

/// Every text value written for `/key`, decoded from the bytes.
pub fn texts_of(bytes: &[u8], key: &str) -> Vec<String> {
    value_tokens(bytes, key.as_bytes()).iter().filter_map(|t| raw_string(t)).map(|b| text_of_raw(&b)).collect()
}

/// A name token's bytes (`/Foo` with `#hh` escapes decoded), or the bare name of a token.
pub fn name_of_token(token: &[u8]) -> Option<Vec<u8>> {
    if *token.first()? != b'/' {
        return None;
    }
    let body = &token[1..];
    let mut out = Vec::new();
    let mut i = 0;
    while i < body.len() {
        if body[i] == b'#' {
            let s = std::str::from_utf8(body.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(s, 16).ok()?);
            i += 3;
        } else {
            out.push(body[i]);
            i += 1;
        }
    }
    Some(out)
}

/// Every name written for `/key`.
pub fn names_of(bytes: &[u8], key: &str) -> Vec<String> {
    value_tokens(bytes, key.as_bytes()).iter().filter_map(|t| name_of_token(t)).map(|n| String::from_utf8_lossy(&n).into_owned()).collect()
}

/// Whether the file holds the name `/name` as its own token, so searching `Rect` cannot match
/// inside `/Rectangle`. It does not know about string objects, so a name-looking sequence inside
/// literal text would also match: use it only for names distinctive enough to be unambiguous.
pub fn has_name(bytes: &[u8], name: &str) -> bool {
    let n = name.as_bytes();
    if n.is_empty() {
        return false;
    }
    let mut i = 0;
    while i + n.len() < bytes.len() {
        if bytes[i] == b'/' && &bytes[i + 1..i + 1 + n.len()] == n {
            let after = i + 1 + n.len();
            if after >= bytes.len() || !is_name_byte(bytes[after]) {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Tokens split on whitespace, with `[` … `]` groups kept whole.
fn split_words(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i] == b'[' {
            let (end, tok) = balanced(bytes, i, b'[', b']').expect("an opened array closes in a token we captured");
            out.push(tok);
            i = end;
            continue;
        }
        let start = i;
        while bytes.get(i).is_some_and(|b| !b.is_ascii_whitespace() && *b != b'[') {
            i += 1;
        }
        out.push(bytes[start..i].to_vec());
    }
    out
}
