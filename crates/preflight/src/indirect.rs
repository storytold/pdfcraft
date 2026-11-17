//! PDF/A-2 and -3 6.1.9 (PDF/A-4 6.1.8): how an indirect object is written in the file.
//!
//! ISO 19005-2/-3 6.1.9 and ISO 19005-4 6.1.8 narrow the syntax of ISO 32000-2 7.3.10:
//!
//! - the object number and the generation number are separated by one white-space character, and
//!   so are the generation number and the `obj` keyword;
//! - the object number is preceded by an end-of-line marker, and `obj` is followed by one;
//! - `endobj` is preceded by an end-of-line marker and followed by one.
//!
//! The white-space characters are those of ISO 32000-2 Table 1, and an end-of-line marker is CR or
//! LF (7.2.3).
//!
//! The objects read are those the cross-reference sections place in the file itself, at the
//! offset given there (counted from the file's start, or from a `%PDF-` header that doesn't start
//! it), or past white space at that offset. An object in an object stream has no such syntax, and one not found at its offset is left
//! out, since where it is written isn't known. An object's `endobj` is looked for between its
//! header and the next object the cross-reference sections place, after the end of its stream
//! data when it is a stream, so each byte of the file is searched at most once.

use pdfcraft_cos::{Document, ObjRef, Object, XrefEntry, is_whitespace};

/// The ways an object can break the rule, each reported once with the first object that does.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Findings {
    /// The numbers and `obj` aren't separated by single white-space characters.
    pub(crate) spacing: Option<u32>,
    /// The object number doesn't follow an end-of-line marker.
    pub(crate) before_header: Option<u32>,
    /// `obj` isn't followed by an end-of-line marker.
    pub(crate) after_obj: Option<u32>,
    /// `endobj` isn't preceded, or isn't followed, by an end-of-line marker.
    pub(crate) around_endobj: Option<u32>,
}

/// How much white space before an object number an offset may point at, as readers allow.
const MAX_LEADING_SPACE: usize = 256;

fn is_eol(b: Option<&u8>) -> bool {
    matches!(b, Some(b'\r' | b'\n'))
}

/// The length of the run of ASCII digits at the start of `data`.
fn digits(data: &[u8]) -> usize {
    data.iter().take_while(|b| b.is_ascii_digit()).count()
}

fn find(data: &[u8], what: &[u8], from: usize, to: usize) -> Option<usize> {
    data.get(from..to)?.windows(what.len()).position(|w| w == what).map(|i| i.saturating_add(from))
}

pub(crate) fn check(doc: &Document) -> Findings {
    let data = doc.bytes().as_slice();
    // Each object in the file, where its header starts.
    let mut placed: Vec<(usize, u32)> = Vec::new();
    for num in doc.object_numbers() {
        let Some(XrefEntry::InFile { offset, .. }) = doc.xref_entry(num) else { continue };
        let Ok(offset) = usize::try_from(offset) else { continue };
        // Where the object number is, when the offset points at it or at white space before it.
        let number_at = |at: usize| {
            let rest = data.get(at..)?;
            let skipped = rest.iter().take(MAX_LEADING_SPACE).take_while(|&&b| is_whitespace(b)).count();
            let rest = rest.get(skipped..)?;
            let n = digits(rest);
            let parsed = rest.get(..n).and_then(|d| std::str::from_utf8(d).ok()).and_then(|d| d.parse::<u32>().ok());
            (n > 0 && parsed == Some(num)).then(|| at.saturating_add(skipped))
        };
        let shifted = || offset.checked_add(doc.header_offset()).filter(|_| doc.header_offset() != 0).and_then(number_at);
        let Some(at) = number_at(offset).or_else(shifted) else { continue };
        placed.push((at, num));
    }
    placed.sort_unstable();
    placed.dedup_by_key(|(at, _)| *at);
    let mut out = Findings::default();
    for (i, &(at, num)) in placed.iter().enumerate() {
        let end = placed.get(i.saturating_add(1)).map_or(data.len(), |&(next, _)| next);
        let note = |slot: &mut Option<u32>| {
            slot.get_or_insert(num);
        };
        let Some(header) = data.get(at..end) else { continue };
        let number = digits(header);
        let (gap_1, comment_1) = header.get(number..).map_or((0, false), space);
        let generation_at = number.saturating_add(gap_1);
        let generation = header.get(generation_at..).map_or(0, digits);
        let gap_2_at = generation_at.saturating_add(generation);
        let (gap_2, comment_2) = header.get(gap_2_at..).map_or((0, false), space);
        let obj_at = gap_2_at.saturating_add(gap_2);
        if generation == 0 || header.get(obj_at..obj_at.saturating_add(3)) != Some(b"obj".as_slice()) {
            // Not an object header after all: nothing more is known of it.
            continue;
        }
        // An object at the very start of the file has no end-of-line marker before it.
        if !is_eol(at.checked_sub(1).and_then(|p| data.get(p))) {
            note(&mut out.before_header);
        }
        // A comment counts as a white-space character of its own (ISO 32000-2 7.2.4), so a gap
        // holding one is never a single white-space character.
        if gap_1 != 1 || gap_2 != 1 || comment_1 || comment_2 {
            note(&mut out.spacing);
        }
        let body = at.saturating_add(obj_at).saturating_add(3);
        if !is_eol(data.get(body)) {
            note(&mut out.after_obj);
        }
        // Where to look for `endobj`: past the stream data, when the object is a stream.
        let mut from = body;
        if let Object::Stream(s) = &*doc.get(ObjRef::new(num, doc.generation(num)))
            && let Some(keyword) = token(data, b"stream", body, end)
        {
            let start = keyword.saturating_add(6);
            let start = start.saturating_add(match data.get(start..start.saturating_add(2)) {
                Some(b"\r\n") => 2,
                Some([b'\r' | b'\n', ..]) => 1,
                _ => 0,
            });
            // The stream data never holds `endstream` itself, so the search starts just inside it.
            let past = start.saturating_add(s.raw.len()).saturating_sub(b"endstream".len()).max(start);
            if let Some(e) = find(data, b"endstream", past.min(end), end) {
                from = e.saturating_add(b"endstream".len());
            }
        }
        let Some(endobj) = token(data, b"endobj", from.min(end), end) else { continue };
        let before = endobj.checked_sub(1).and_then(|p| data.get(p));
        let after = data.get(endobj.saturating_add(6));
        // The last object of a file without a cross-reference table after it may end the file.
        if !is_eol(before) || (after.is_some() && !is_eol(after)) {
            note(&mut out.around_endobj);
        }
    }
    out
}

/// The white space and comments at the start of `data`: how many bytes they take, and whether a
/// comment is among them.
fn space(data: &[u8]) -> (usize, bool) {
    let mut at = 0usize;
    let mut comment = false;
    while let Some(&b) = data.get(at) {
        if is_whitespace(b) {
            at = at.saturating_add(1);
        } else if b == b'%' {
            comment = true;
            at = at.saturating_add(comment_len(data.get(at..).unwrap_or_default()));
        } else {
            break;
        }
    }
    (at, comment)
}

/// The length of the comment at the start of `data`, up to its end-of-line marker.
fn comment_len(data: &[u8]) -> usize {
    data.iter().position(|&b| matches!(b, b'\r' | b'\n')).unwrap_or(data.len())
}

fn is_delimiter(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// Where the first `keyword` token in `from..to` starts. Literal and hexadecimal strings, names
/// and comments are skipped, so the keyword's bytes inside one of them don't count (ISO 32000-2
/// 7.2 and 7.3).
fn token(data: &[u8], keyword: &[u8], from: usize, to: usize) -> Option<usize> {
    let span = data.get(from..to)?;
    let mut at = 0usize;
    while let Some(&b) = span.get(at) {
        let rest = span.get(at..).unwrap_or_default();
        let len = match b {
            b'%' => comment_len(rest),
            b'(' => literal_len(rest),
            // `<<` opens a dictionary; a single `<` a hexadecimal string, up to its `>`.
            b'<' if rest.get(1) == Some(&b'<') => 2,
            b'<' => rest.iter().position(|&c| c == b'>').map_or(rest.len(), |p| p.saturating_add(1)),
            b'/' => rest.iter().skip(1).take_while(|&&c| !is_whitespace(c) && !is_delimiter(c)).count().saturating_add(1),
            _ if is_whitespace(b) || is_delimiter(b) => 1,
            _ => {
                let n = rest.iter().take_while(|&&c| !is_whitespace(c) && !is_delimiter(c)).count();
                if rest.get(..n) == Some(keyword) {
                    return Some(from.saturating_add(at));
                }
                n
            }
        };
        at = at.saturating_add(len.max(1));
    }
    None
}

/// The length of the literal string at the start of `data`, up to its balancing `)` (7.3.4.2); all
/// of `data` when it isn't closed.
fn literal_len(data: &[u8]) -> usize {
    let mut depth = 0u32;
    let mut at = 0usize;
    while let Some(&b) = data.get(at) {
        at = at.saturating_add(1);
        match b {
            // The escaped byte is never a parenthesis that counts.
            b'\\' => at = at.saturating_add(1),
            b'(' => depth = depth.saturating_add(1),
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return at;
                }
            }
            _ => {}
        }
    }
    data.len()
}
