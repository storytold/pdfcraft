//! A small BER/DER reader and writer (ITU-T X.690): just what X.509, CMS and PKCS#12 need.
//!
//! [`Tlv`] borrows from the input and keeps its full encoding (`raw`), so signed structures can
//! be verified over the exact bytes that were signed.
//!
//! Reading is strict DER by default, which is what everything signed must use. [`Tlv::parse_ber`]
//! also accepts the two BER constructions a reader meets in practice — the indefinite length form
//! and strings split into segments — which PKCS #12 needs: Windows writes `.p12` files that way.
//! Writing is always DER.

use std::borrow::Cow;

use crate::SignError;

/// Universal tags (single-byte identifiers).
pub mod tag {
    pub const BOOLEAN: u8 = 0x01;
    pub const INTEGER: u8 = 0x02;
    pub const BIT_STRING: u8 = 0x03;
    pub const OCTET_STRING: u8 = 0x04;
    /// An OCTET STRING split into segments (BER only, X.690 §8.21); see [`super::Tlv::octets`].
    pub const OCTET_STRING_SEGMENTS: u8 = 0x24;
    pub const NULL: u8 = 0x05;
    pub const OID: u8 = 0x06;
    pub const UTF8_STRING: u8 = 0x0C;
    pub const PRINTABLE_STRING: u8 = 0x13;
    pub const T61_STRING: u8 = 0x14;
    pub const IA5_STRING: u8 = 0x16;
    pub const UTC_TIME: u8 = 0x17;
    pub const GENERALIZED_TIME: u8 = 0x18;
    pub const BMP_STRING: u8 = 0x1E;
    pub const SEQUENCE: u8 = 0x30;
    pub const SET: u8 = 0x31;
    /// `[n]` context-specific, constructed (EXPLICIT, or IMPLICIT over a constructed type).
    pub const fn ctx(n: u8) -> u8 {
        0xA0 | n
    }
    /// `[n]` context-specific, primitive (IMPLICIT over a primitive type).
    pub const fn ctx_prim(n: u8) -> u8 {
        0x80 | n
    }
}

fn bad(what: &str) -> SignError {
    SignError::Malformed(what.to_string())
}

/// Which encoding rules a reader accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// DER: every length definite, every string primitive. Anything whose bytes are signed is read
    /// this way, so that what is verified is exactly what was written.
    Der,
    /// BER as well: the indefinite length form (X.690 §8.1.3.6) and strings split into segments
    /// (§8.21).
    Ber,
}

/// One BER/DER element.
#[derive(Clone, Copy, Debug, Eq)]
pub struct Tlv<'a> {
    pub tag: u8,
    /// The contents octets (without the end-of-contents octets of an indefinite length).
    pub value: &'a [u8],
    /// The whole encoding (identifier, length, contents and any end-of-contents octets).
    pub raw: &'a [u8],
    /// The rules this element was read with. Its children are read the same way.
    form: Form,
}

/// Two elements are equal when they encode the same thing; the rules they were read with are not
/// part of that.
impl PartialEq for Tlv<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.tag == other.tag && self.value == other.value && self.raw == other.raw
    }
}

impl<'a> Tlv<'a> {
    /// Parse one DER element at the start of `input`; returns it and the rest.
    pub fn parse(input: &'a [u8]) -> Result<(Tlv<'a>, &'a [u8]), SignError> {
        Tlv::read(input, Form::Der, 0)
    }

    /// Parse one BER element at the start of `input`; returns it and the rest.
    pub fn parse_ber(input: &'a [u8]) -> Result<(Tlv<'a>, &'a [u8]), SignError> {
        Tlv::read(input, Form::Ber, 0)
    }

    fn read(input: &'a [u8], form: Form, depth: usize) -> Result<(Tlv<'a>, &'a [u8]), SignError> {
        let (&tag, rest) = input.split_first().ok_or_else(|| bad("truncated DER"))?;
        if tag & 0x1F == 0x1F {
            return Err(bad("multi-byte DER tags are not supported"));
        }
        let (&first, rest) = rest.split_first().ok_or_else(|| bad("truncated DER length"))?;
        // `trailer` is the two end-of-contents octets, which belong to the element but not to its
        // contents.
        let (len, rest, trailer) = if first < 0x80 {
            (first as usize, rest, 0)
        } else if first == 0x80 {
            if form == Form::Der {
                return Err(bad("indefinite lengths are not allowed in DER"));
            }
            if tag & 0x20 == 0 {
                return Err(bad("an indefinite length needs a constructed element"));
            }
            (contents_len(rest, form, depth)?, rest, 2)
        } else {
            let n = (first & 0x7F) as usize;
            if n > 4 || rest.len() < n {
                return Err(bad("unsupported DER length"));
            }
            let len = rest[..n].iter().fold(0usize, |acc, b| (acc << 8) | *b as usize);
            (len, &rest[n..], 0)
        };
        let end = len.checked_add(trailer).ok_or_else(|| bad("DER length overflows"))?;
        if rest.len() < end {
            return Err(bad("DER element runs past the end"));
        }
        // `end <= rest.len()`, so neither sum below can overflow.
        let header = input.len() - rest.len();
        Ok((Tlv { tag, value: &rest[..len], raw: &input[..header + end], form }, &rest[end..]))
    }

    /// Parse `input` as exactly one DER element.
    pub fn parse_all(input: &'a [u8]) -> Result<Tlv<'a>, SignError> {
        Tlv::only(input, Form::Der)
    }

    /// Parse `input` as exactly one BER element.
    pub fn parse_all_ber(input: &'a [u8]) -> Result<Tlv<'a>, SignError> {
        Tlv::only(input, Form::Ber)
    }

    fn only(input: &'a [u8], form: Form) -> Result<Tlv<'a>, SignError> {
        let (t, rest) = Tlv::read(input, form, 0)?;
        if !rest.is_empty() {
            return Err(bad("trailing bytes after DER element"));
        }
        Ok(t)
    }

    /// Whether this element is written with the indefinite length form (BER, never DER). Tags are
    /// single-octet here, so the length octet is the second.
    pub fn is_indefinite(&self) -> bool {
        self.raw.get(1) == Some(&0x80)
    }

    /// The children of a constructed element, read with the same rules as their parent.
    pub fn children(&self) -> Result<Vec<Tlv<'a>>, SignError> {
        let mut out = Vec::new();
        let mut rest = self.value;
        while !rest.is_empty() {
            let (t, r) = Tlv::read(rest, self.form, 0)?;
            out.push(t);
            rest = r;
        }
        Ok(out)
    }

    /// Check the tag, for a clear error.
    pub fn expect(self, tag: u8, what: &str) -> Result<Tlv<'a>, SignError> {
        if self.tag == tag { Ok(self) } else { Err(bad(&format!("{what}: expected tag {tag:#04x}, found {:#04x}", self.tag))) }
    }

    /// The element inside an EXPLICIT tag.
    pub fn inner(&self) -> Result<Tlv<'a>, SignError> {
        Tlv::only(self.value, self.form)
    }

    /// An OCTET STRING's contents, joining the segments of BER's constructed form (X.690 §8.21).
    /// Borrowed in the ordinary case, so the common path copies nothing.
    pub fn octets(&self, what: &str) -> Result<Cow<'a, [u8]>, SignError> {
        self.join(what, 0)
    }

    fn join(&self, what: &str, depth: usize) -> Result<Cow<'a, [u8]>, SignError> {
        /// Deep enough for anything real: a writer that segments at all segments once.
        const MAX_DEPTH: usize = 16;
        match self.tag {
            tag::OCTET_STRING => Ok(Cow::Borrowed(self.value)),
            tag::OCTET_STRING_SEGMENTS if self.form == Form::Ber => {
                if depth >= MAX_DEPTH {
                    return Err(bad(&format!("{what}: OCTET STRING segments nested too deeply")));
                }
                // The join is never larger than the segments it is made of, so this is bounded by
                // the input.
                let mut out: Vec<u8> = Vec::with_capacity(self.value.len());
                for s in self.children()? {
                    out.extend_from_slice(&s.join(what, depth + 1)?);
                }
                Ok(Cow::Owned(out))
            }
            found => Err(bad(&format!("{what}: expected an OCTET STRING, found tag {found:#04x}"))),
        }
    }

    /// An OID in dotted form.
    pub fn oid(&self) -> Result<String, SignError> {
        if self.tag != tag::OID {
            return Err(bad("expected an OID"));
        }
        oid_to_string(self.value)
    }

    /// A non-negative INTEGER that fits in a u64.
    pub fn u64(&self) -> Result<u64, SignError> {
        if self.tag != tag::INTEGER || self.value.is_empty() || self.value.len() > 9 || self.value[0] & 0x80 != 0 {
            return Err(bad("expected a small non-negative INTEGER"));
        }
        Ok(self.value.iter().fold(0u64, |acc, b| (acc << 8) | *b as u64))
    }

    /// The magnitude of an INTEGER (leading zero dropped).
    pub fn uint_bytes(&self) -> &'a [u8] {
        match self.value {
            [0, rest @ ..] if !rest.is_empty() => rest,
            v => v,
        }
    }

    /// A BIT STRING's bytes (no unused bits allowed).
    pub fn bits(&self) -> Result<&'a [u8], SignError> {
        match self.value {
            [0, rest @ ..] if self.tag == tag::BIT_STRING => Ok(rest),
            _ => Err(bad("expected a whole-byte BIT STRING")),
        }
    }

    /// A string type as text (UTF-8, Printable, IA5, T61 as Latin-1, BMP).
    pub fn text(&self) -> Option<String> {
        match self.tag {
            tag::UTF8_STRING | tag::PRINTABLE_STRING | tag::IA5_STRING => Some(String::from_utf8_lossy(self.value).into_owned()),
            tag::T61_STRING => Some(self.value.iter().map(|b| *b as char).collect()),
            tag::BMP_STRING => {
                let units: Vec<u16> = self.value.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                Some(String::from_utf16_lossy(&units))
            }
            _ => None,
        }
    }

    /// UTCTime / GeneralizedTime as (year, month, day, hour, minute, second) in UTC.
    pub fn time(&self) -> Result<Time, SignError> {
        let s = std::str::from_utf8(self.value).map_err(|_| bad("time"))?;
        let s = s.strip_suffix('Z').ok_or_else(|| bad("times must be in UTC (Z)"))?;
        let num = |r: std::ops::Range<usize>| s.get(r).and_then(|x| x.parse::<u32>().ok()).ok_or_else(|| bad("time"));
        let (year, rest) = match self.tag {
            tag::UTC_TIME => {
                let y = num(0..2)?;
                (if y >= 50 { 1900 + y } else { 2000 + y }, 2)
            }
            tag::GENERALIZED_TIME => (num(0..4)?, 4),
            _ => return Err(bad("expected a time")),
        };
        let t = Time {
            year,
            month: num(rest..rest + 2)?,
            day: num(rest + 2..rest + 4)?,
            hour: num(rest + 4..rest + 6)?,
            minute: num(rest + 6..rest + 8)?,
            second: if s.len() >= rest + 10 { num(rest + 8..rest + 10)? } else { 0 },
        };
        Ok(t)
    }
}

/// How many contents octets an indefinite-length element has: everything up to the matching
/// end-of-contents octets, which have to be there (X.690 §8.1.5). The contents are elements, so
/// finding the end means reading them, and one of those may be indefinite too — hence the depth
/// limit. Every step consumes at least two octets, so the walk always ends.
fn contents_len(input: &[u8], form: Form, depth: usize) -> Result<usize, SignError> {
    /// Far deeper than any real file nests, and shallow enough never to exhaust the stack.
    const MAX_DEPTH: usize = 48;
    if depth >= MAX_DEPTH {
        return Err(bad("BER elements nested too deeply"));
    }
    let mut rest = input;
    loop {
        match rest {
            [] => return Err(bad("an indefinite length with no end-of-contents")),
            [0, 0, ..] => return Ok(input.len() - rest.len()),
            _ => rest = Tlv::read(rest, form, depth + 1)?.1,
        }
    }
}

/// A UTC time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Time {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Time {
    /// From a PDF date (`D:YYYYMMDDHHmmSS±HH'mm'`), converted to UTC.
    pub fn from_pdf(s: &str) -> Option<Time> {
        let s = s.strip_prefix("D:").unwrap_or(s);
        let d = |r: std::ops::Range<usize>, default: u32| s.get(r).map_or(Some(default), |x| x.parse::<u32>().ok());
        let mut t =
            Time { year: d(0..4, 0)?, month: d(4..6, 1)?, day: d(6..8, 1)?, hour: d(8..10, 0)?, minute: d(10..12, 0)?, second: d(12..14, 0)? };
        if t.year == 0 {
            return None;
        }
        let tz = s.get(14..).unwrap_or("");
        let sign = match tz.chars().next() {
            Some('+') => -1i64,
            Some('-') => 1,
            _ => 0,
        };
        if sign != 0 {
            let h = tz.get(1..3).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
            let m = tz.get(4..6).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
            t = t.add_seconds(sign * (h * 3600 + m * 60));
        }
        Some(t)
    }

    /// As a PDF date in UTC (`D:YYYYMMDDHHmmSSZ`).
    pub fn to_pdf(&self) -> String {
        format!("D:{:04}{:02}{:02}{:02}{:02}{:02}Z", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }

    /// Seconds since 1970-01-01 (proleptic Gregorian).
    pub fn unix(&self) -> i64 {
        let (y, m) = if self.month <= 2 { (self.year as i64 - 1, self.month as i64 + 9) } else { (self.year as i64, self.month as i64 - 3) };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * m + 2) / 5 + self.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        days * 86_400 + self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64
    }

    pub fn from_unix(t: i64) -> Time {
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = (yoe + era * 400 + if month <= 2 { 1 } else { 0 }) as u32;
        Time { year, month, day, hour: (secs / 3600) as u32, minute: (secs % 3600 / 60) as u32, second: (secs % 60) as u32 }
    }

    pub fn add_seconds(&self, s: i64) -> Time {
        Time::from_unix(self.unix() + s)
    }

    /// UTCTime before 2050, GeneralizedTime from then on (RFC 5280 §4.1.2.5).
    pub fn encode(&self) -> Vec<u8> {
        if (1950..2050).contains(&self.year) {
            let s = format!("{:02}{:02}{:02}{:02}{:02}{:02}Z", self.year % 100, self.month, self.day, self.hour, self.minute, self.second);
            tlv(tag::UTC_TIME, s.as_bytes())
        } else {
            let s = format!("{:04}{:02}{:02}{:02}{:02}{:02}Z", self.year, self.month, self.day, self.hour, self.minute, self.second);
            tlv(tag::GENERALIZED_TIME, s.as_bytes())
        }
    }
}

impl std::fmt::Display for Time {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}.{:02}.{:02} {:02}:{:02}:{:02} UTC", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }
}

fn oid_to_string(v: &[u8]) -> Result<String, SignError> {
    let mut arcs: Vec<u64> = Vec::new();
    let mut acc = 0u64;
    for (i, b) in v.iter().enumerate() {
        acc = acc.checked_mul(128).ok_or_else(|| bad("OID arc too large"))? | (b & 0x7F) as u64;
        if b & 0x80 == 0 {
            if arcs.is_empty() {
                let first = (acc / 40).min(2);
                arcs.push(first);
                arcs.push(acc - first * 40);
            } else {
                arcs.push(acc);
            }
            acc = 0;
        } else if i + 1 == v.len() {
            return Err(bad("truncated OID"));
        }
    }
    Ok(arcs.iter().map(u64::to_string).collect::<Vec<_>>().join("."))
}

// ── writing ─────────────────────────────────────────────────────────────────────────────────

/// One element: identifier, definite length, contents.
pub fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = value.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes: Vec<u8> = n.to_be_bytes().iter().copied().skip_while(|b| *b == 0).collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend(bytes);
    }
    out.extend_from_slice(value);
    out
}

pub fn seq(parts: &[&[u8]]) -> Vec<u8> {
    tlv(tag::SEQUENCE, &parts.concat())
}

/// A SET OF, in DER order (sorted encodings, X.690 §11.6).
pub fn set_of(parts: &[&[u8]]) -> Vec<u8> {
    let mut v: Vec<&[u8]> = parts.to_vec();
    v.sort();
    tlv(tag::SET, &v.concat())
}

pub fn explicit(n: u8, inner: &[u8]) -> Vec<u8> {
    tlv(tag::ctx(n), inner)
}

/// The encoding of an OID constant (callers pass only literals from this crate).
// The documented never-crash exception: every caller passes a literal that always parses.
#[allow(clippy::expect_used)]
pub fn oid(dotted: &str) -> Vec<u8> {
    let arcs: Vec<u64> = dotted.split('.').map(|a| a.parse().expect("valid OID constant")).collect();
    let mut body = Vec::new();
    let mut push = |mut v: u64| {
        let mut tmp = vec![(v & 0x7F) as u8];
        v >>= 7;
        while v > 0 {
            tmp.push(0x80 | (v & 0x7F) as u8);
            v >>= 7;
        }
        tmp.reverse();
        body.extend(tmp);
    };
    push(arcs[0] * 40 + arcs[1]);
    for a in &arcs[2..] {
        push(*a);
    }
    tlv(tag::OID, &body)
}

/// Encode a dotted OID supplied by a caller. Unlike [`oid`], this validates every arc and
/// never panics on malformed input.
pub fn try_oid(dotted: &str) -> Result<Vec<u8>, SignError> {
    let mut arcs = dotted.split('.').map(|a| a.parse::<u64>().map_err(|_| bad("invalid OID arc")));
    let first = arcs.next().ok_or_else(|| bad("empty OID"))??;
    let second = arcs.next().ok_or_else(|| bad("OID needs two arcs"))??;
    if first > 2 || (first < 2 && second >= 40) {
        return Err(bad("invalid OID prefix"));
    }
    let mut body = Vec::new();
    let mut push = |mut v: u64| {
        let mut tmp = vec![(v & 0x7F) as u8];
        v >>= 7;
        while v > 0 {
            tmp.push(0x80 | (v & 0x7F) as u8);
            v >>= 7;
        }
        tmp.reverse();
        body.extend(tmp);
    };
    let base = first.checked_mul(40).and_then(|b| b.checked_add(second)).ok_or_else(|| bad("invalid OID prefix"))?;
    push(base);
    for arc in arcs {
        push(arc?);
    }
    Ok(tlv(tag::OID, &body))
}
pub fn uint(bytes: &[u8]) -> Vec<u8> {
    let b: &[u8] = match bytes.iter().position(|x| *x != 0) {
        Some(i) => &bytes[i..],
        None => &[0],
    };
    if b[0] & 0x80 != 0 {
        let mut v = vec![0];
        v.extend_from_slice(b);
        tlv(tag::INTEGER, &v)
    } else {
        tlv(tag::INTEGER, b)
    }
}

pub fn int(n: u64) -> Vec<u8> {
    uint(&n.to_be_bytes())
}

pub fn octets(b: &[u8]) -> Vec<u8> {
    tlv(tag::OCTET_STRING, b)
}

pub fn bit_string(b: &[u8]) -> Vec<u8> {
    let mut v = vec![0];
    v.extend_from_slice(b);
    tlv(tag::BIT_STRING, &v)
}

pub fn null() -> Vec<u8> {
    vec![tag::NULL, 0]
}

pub fn utf8(s: &str) -> Vec<u8> {
    tlv(tag::UTF8_STRING, s.as_bytes())
}

pub fn printable(s: &str) -> Vec<u8> {
    tlv(tag::PRINTABLE_STRING, s.as_bytes())
}

pub fn ia5(s: &str) -> Vec<u8> {
    tlv(tag::IA5_STRING, s.as_bytes())
}

pub fn boolean(b: bool) -> Vec<u8> {
    tlv(tag::BOOLEAN, &[if b { 0xFF } else { 0 }])
}

/// `AlgorithmIdentifier` with optional parameters.
pub fn algorithm(oid_s: &str, params: Option<&[u8]>) -> Vec<u8> {
    match params {
        Some(p) => seq(&[&oid(oid_s), p]),
        None => seq(&[&oid(oid_s)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let o = oid("1.2.840.113549.1.7.2");
        let t = Tlv::parse_all(&o).unwrap();
        assert_eq!(t.oid().unwrap(), "1.2.840.113549.1.7.2");
        let big = vec![7u8; 300];
        let e = octets(&big);
        assert_eq!(&e[..4], &[0x04, 0x82, 0x01, 0x2C]);
        assert_eq!(Tlv::parse_all(&e).unwrap().value, &big[..]);
        assert_eq!(uint(&[0x80, 1]), vec![0x02, 3, 0, 0x80, 1]);
        assert_eq!(Tlv::parse_all(&int(65537)).unwrap().u64().unwrap(), 65537);
        let s = set_of(&[&int(2), &int(1)]);
        assert_eq!(s, vec![0x31, 6, 2, 1, 1, 2, 1, 2]);
        assert!(Tlv::parse(&[0x30, 5, 1]).is_err());
    }

    #[test]
    fn reads_ber_indefinite_lengths_and_segmented_strings() {
        // SEQUENCE { INTEGER 3, OCTET STRING { "hi", "!" } }, every wrapper in the indefinite form:
        // the shape Windows gives a .p12 file.
        let ber: &[u8] = &[0x30, 0x80, 0x02, 0x01, 0x03, 0x24, 0x80, 0x04, 0x02, b'h', b'i', 0x04, 0x01, b'!', 0, 0, 0, 0];
        let e = Tlv::parse_all(ber).unwrap_err().to_string();
        assert!(e.contains("indefinite lengths are not allowed in DER"), "{e}");

        let t = Tlv::parse_all_ber(ber).unwrap();
        assert_eq!(t.tag, tag::SEQUENCE);
        assert_eq!(t.raw, ber, "the end-of-contents octets belong to the element");
        assert_eq!(t.value, &ber[2..ber.len() - 2], "but not to its contents");
        let c = t.children().unwrap();
        assert_eq!(c[0].u64().unwrap(), 3);
        assert_eq!(c[1].octets("segments").unwrap().as_ref(), b"hi!", "the segments are joined");
        assert!(c[0].octets("not a string").is_err());

        // Definite lengths read the same either way, and a segmented string stays a DER error.
        assert_eq!(Tlv::parse_all_ber(&int(65537)).unwrap().u64().unwrap(), 65537);
        assert_eq!(Tlv::parse_all_ber(&octets(b"hi!")).unwrap().octets("plain").unwrap().as_ref(), b"hi!");
        assert!(Tlv::parse_all(&[0x24, 2, 0x04, 0]).unwrap().octets("segments").is_err());

        // Malformed: no end-of-contents, and an indefinite length on a primitive element.
        assert!(Tlv::parse_all_ber(&ber[..ber.len() - 2]).is_err());
        assert!(Tlv::parse_all_ber(&[0x04, 0x80, 0, 0]).is_err());
        // Nesting is bounded, so a file made of nothing but wrappers fails instead of crashing.
        let deep = [[0x30u8, 0x80].repeat(400), [0u8, 0].repeat(400)].concat();
        assert!(Tlv::parse_all_ber(&deep).is_err());
    }

    #[test]
    fn times() {
        let t = Time { year: 2026, month: 10, day: 2, hour: 9, minute: 30, second: 5 };
        assert_eq!(Time::from_unix(t.unix()), t);
        assert_eq!(Tlv::parse_all(&t.encode()).unwrap().time().unwrap(), t);
        let far = Time { year: 2051, ..t };
        assert_eq!(Tlv::parse_all(&far.encode()).unwrap().tag, tag::GENERALIZED_TIME);
        assert_eq!(Time::from_pdf("D:20261002113005+02'00'"), Some(Time { year: 2026, month: 10, day: 2, hour: 9, minute: 30, second: 5 }));
        assert_eq!(Time::from_unix(0), Time { year: 1970, month: 1, day: 1, hour: 0, minute: 0, second: 0 });
    }
}
