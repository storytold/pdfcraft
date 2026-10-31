//! Shared helpers of the proof: what one proof may spend, text and byte search, and the
//! spellings a string can have in a PDF.

use std::cell::Cell;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;

use pdfcraft_cos::{DecodeBudget, Dict, Document, Object, Stream};
use sha2::{Digest, Sha256};

use crate::verify::limits::{MAX_STREAM, MAX_TOTAL_DECODED, MAX_WORK, MIN_NEEDLE};

/// What one proof may spend, and what it knows about the document it checks.
pub(super) struct Ctx {
    /// Digests of the decoded content of the overlay streams (their boxes and labels are drawn
    /// on purpose and are neither text to look for nor paint to flag).
    pub(super) overlay: HashSet<String>,
    pub(super) decoded: Cell<u64>,
    pub(super) work: Cell<u64>,
}

impl Ctx {
    pub(super) fn new(overlay: HashSet<String>) -> Self {
        Ctx { overlay, decoded: Cell::new(0), work: Cell::new(0) }
    }

    /// Decode `s` strictly: a stream that is damaged, longer than [`MAX_STREAM`], or one more than
    /// [`MAX_TOTAL_DECODED`] allows is an error (never silently truncated data).
    pub(super) fn decode(&self, s: &Stream) -> Result<Vec<u8>, ()> {
        let left = MAX_TOTAL_DECODED.saturating_sub(self.decoded.get());
        let cap = usize::try_from(left).unwrap_or(usize::MAX).min(MAX_STREAM);
        // Strict and within the cap, charged to a one-stream allowance (what is left of the
        // proof's decode budget is tracked here, by decoded output length).
        let data = DecodeBudget::new(cap, usize::MAX).decode_strict(s).map_err(|_| ())?;
        // The decode is bounded by the cap; the check keeps an over-long stream an error
        // whatever the reader does.
        if data.len() > cap {
            return Err(());
        }
        self.decoded.set(self.decoded.get().saturating_add(data.len() as u64));
        Ok(data)
    }

    /// Spend `n` units of search work; false once [`MAX_WORK`] is used up.
    pub(super) fn charge(&self, n: usize) -> bool {
        let used = self.work.get().saturating_add(n as u64);
        self.work.set(used);
        used <= MAX_WORK
    }

    /// Is this decoded content one of the overlay streams this run created?
    pub(super) fn is_overlay(&self, data: &[u8]) -> bool {
        !self.overlay.is_empty() && self.overlay.contains(&sha256(&[data]))
    }
}

/// Follow `o` to its object; a reference that can't be loaded is an error (a missing object is
/// `Null` for [`Document::resolve`], which would read as "empty").
pub(super) fn resolve_strict(doc: &Document, o: &Object) -> Result<Arc<Object>, ()> {
    match o {
        Object::Ref(r) => doc.try_get(r.num).map_err(|_| ()),
        other => Ok(Arc::new(other.clone())),
    }
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

pub(super) fn sha256<T: AsRef<[u8]>>(parts: &[T]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        let p = p.as_ref();
        h.update((p.len() as u64).to_be_bytes());
        h.update(p);
    }
    hex(h.finalize().as_slice())
}

/// The text of `s` without whitespace (page text is compared whitespace-blind: a gap in a `TJ`
/// is a space in one extraction and nothing in another).
pub(super) fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace() && *c != '\u{feff}').collect()
}

pub(super) fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// The words of a run worth looking for on their own (a run that survives in pieces).
pub(super) fn words(s: &str) -> Vec<&str> {
    s.split_whitespace().filter(|w| w.chars().count() >= MIN_NEEDLE).collect()
}

pub(super) fn count(hay: &str, needle: &str) -> usize {
    if needle.is_empty() { 0 } else { hay.matches(needle).count() }
}

/// Is `pat` in `hay`?
pub(super) fn find(hay: &[u8], pat: &[u8]) -> bool {
    find_from(hay, pat, 0).is_some()
}

/// Where `pat` first occurs in `hay` at or after `from`. The pattern's prefix function is
/// built once and the haystack is walked once: the search is worst-case linear, so
/// adversarial input (a needle of one letter and a haystack full of it) can neither make it
/// quadratic nor spend more comparisons than the caller charges for.
pub(super) fn find_from(hay: &[u8], pat: &[u8], from: usize) -> Option<usize> {
    let m = pat.len();
    if m == 0 {
        return None;
    }
    // fail[i]: the longest proper prefix of `pat` that ends at `i` and is also a suffix there.
    let mut fail = vec![0usize; m];
    let mut k = 0;
    for (i, &c) in pat.iter().enumerate().skip(1) {
        while k > 0 && pat[k] != c {
            k = fail[k - 1];
        }
        if pat[k] == c {
            k += 1;
        }
        fail[i] = k;
    }
    let mut k = 0;
    for (at, &c) in hay.iter().enumerate().skip(from) {
        while k > 0 && pat[k] != c {
            k = fail[k - 1];
        }
        if pat[k] == c {
            k += 1;
            if k == m {
                return Some(at + 1 - m);
            }
        }
    }
    None
}

/// Push the span of every non-overlapping occurrence of `pat` in `hay` (at most `cap` in all).
pub(super) fn find_all(hay: &[u8], pat: &[u8], cap: usize, spans: &mut Vec<(usize, usize)>) {
    let mut at = 0;
    while spans.len() < cap {
        let Some(start) = find_from(hay, pat, at) else { return };
        let end = start + pat.len();
        spans.push((start, end));
        at = end;
    }
}

pub(super) fn escape_literal(b: &[u8], octal: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len() * 2);
    for &c in b {
        if octal {
            out.extend(format!("\\{c:03o}").bytes());
            continue;
        }
        match c {
            b'(' | b')' | b'\\' => out.extend([b'\\', c]),
            b'\n' => out.extend(*b"\\n"),
            b'\r' => out.extend(*b"\\r"),
            b'\t' => out.extend(*b"\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// Every way `text` can be spelled in a PDF: UTF-8, UTF-16BE (with and without a byte order
/// mark) and UTF-16LE, each as raw bytes, as a hex string (either case) and as a backslash-
/// escaped literal string (minimal escapes, and octal for every byte).
pub(crate) fn survivor_encodings(text: &str) -> Vec<Vec<u8>> {
    let utf16 = |be: bool, bom: bool| -> Vec<u8> {
        let mut v = Vec::new();
        if bom {
            v.extend(if be { [0xFE, 0xFF] } else { [0xFF, 0xFE] });
        }
        for u in text.encode_utf16() {
            v.extend(if be { u.to_be_bytes() } else { u.to_le_bytes() });
        }
        v
    };
    let forms = [text.as_bytes().to_vec(), utf16(true, true), utf16(true, false), utf16(false, true), utf16(false, false)];
    spellings(forms)
}

/// The spellings of a byte string (see [`survivor_encodings`]).
pub(super) fn spellings(forms: impl IntoIterator<Item = Vec<u8>>) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    for f in forms {
        if f.is_empty() {
            continue;
        }
        let h = hex(&f);
        out.push(h.to_uppercase().into_bytes());
        out.push(h.into_bytes());
        out.push(escape_literal(&f, false));
        out.push(escape_literal(&f, true));
        out.push(f);
    }
    let mut seen = HashSet::new();
    out.retain(|b| seen.insert(b.clone()));
    out
}

/// Text of a payload: UTF-16 when it has a byte order mark, else UTF-8 (lossy).
pub(super) fn decode_text(b: &[u8]) -> String {
    let units = |be: bool, rest: &[u8]| -> String {
        let u: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| if be { u16::from_be_bytes(*c) } else { u16::from_le_bytes(*c) }).collect();
        String::from_utf16_lossy(&u)
    };
    match b {
        [0xFE, 0xFF, rest @ ..] => units(true, rest),
        [0xFF, 0xFE, rest @ ..] => units(false, rest),
        _ => String::from_utf8_lossy(b).into_owned(),
    }
}

pub(super) fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or_default());
        rest = rest.get(at..).unwrap_or_default();
        match xml_entity(rest) {
            Some((c, len)) => {
                out.push(c);
                rest = rest.get(len..).unwrap_or_default();
            }
            None => {
                out.push('&');
                rest = rest.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    out
}

/// The entity starting at `&`: the character it reads as, and how far it runs (through the
/// closing `;`). A numeric reference may carry any number of leading zeros, so its digits
/// are padded without a bound; more significant digits than a character value can hold, or
/// a missing `;`, is not an entity and the `&` stays as it is. Walking only the digits the
/// reference itself holds keeps the decode linear in the input.
fn xml_entity(s: &str) -> Option<(char, usize)> {
    for (name, c) in [("&amp;", '&'), ("&lt;", '<'), ("&gt;", '>'), ("&quot;", '"'), ("&apos;", '\'')] {
        if s.starts_with(name) {
            return Some((c, name.len()));
        }
    }
    let body = s.strip_prefix("&#")?;
    let (radix, digits) = match body.as_bytes().first() {
        Some(b'x' | b'X') => (16u32, &body[1..]),
        _ => (10, body),
    };
    let mut value: u32 = 0;
    let mut digits_held = false;
    let mut end = None;
    for (i, c) in digits.char_indices() {
        match c.to_digit(radix) {
            Some(d) => {
                digits_held = true;
                // Leading zeros only pad; past a character's worth of significant digits the
                // multiplication overflows and this was never an entity.
                value = value.checked_mul(radix)?.checked_add(d)?;
            }
            None => {
                if c == ';' && digits_held {
                    end = Some(i);
                }
                break;
            }
        }
    }
    let end = end?;
    let ch = char::from_u32(value)?;
    Some((ch, 2 + usize::from(radix == 16) + end + 1))
}

/// Names of the filters of a stream dictionary.
pub(super) fn filter_names(d: &Dict) -> Vec<Vec<u8>> {
    match d.get(b"Filter") {
        Some(Object::Name(n)) => vec![n.clone()],
        Some(Object::Array(a)) => a.iter().filter_map(|o| o.as_name().map(<[u8]>::to_vec)).collect(),
        _ => Vec::new(),
    }
}

/// Image codecs: their data is pixels, still encoded.
pub(super) fn image_codec(d: &Dict) -> bool {
    filter_names(d).iter().any(|n| matches!(n.as_slice(), b"DCTDecode" | b"DCT" | b"JPXDecode" | b"CCITTFaxDecode" | b"CCF" | b"JBIG2Decode"))
}
