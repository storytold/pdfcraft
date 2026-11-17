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

use pdfcraft_cos::{Document, Lexer, ObjRef, Object, XrefEntry, is_whitespace};

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
        // An object with an unsaved edit is written again when the file is saved, so its old
        // bytes aren't what is judged; the document's copy of it isn't what they hold either.
        if doc.is_edited(num) {
            continue;
        }
        let note = |slot: &mut Option<u32>| {
            slot.get_or_insert(num);
        };
        let Some(header) = data.get(at..end) else { continue };
        let number = digits(header);
        let (gap_1, comment_1) = space(header, number);
        let generation_at = number.saturating_add(gap_1);
        let generation = header.get(generation_at..).map_or(0, digits);
        let gap_2_at = generation_at.saturating_add(generation);
        let (gap_2, comment_2) = space(header, gap_2_at);
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
        // Where to look for `endobj`: past the stream data and any comments after `endstream`
        // when the object is a stream, else past its value and any comments after it, so `endobj`
        // in a string, name or comment isn't taken for it.
        let past = match &*doc.get(ObjRef::new(num, doc.generation(num))) {
            Object::Stream(s) => past_stream(data, body, end, s.raw.len()),
            _ => None,
        };
        let from = past.or_else(|| past_value(data, body, end)).unwrap_or(body);
        let Some(endobj) = find(data, b"endobj", from.min(end), end) else { continue };
        let before = endobj.checked_sub(1).and_then(|p| data.get(p));
        let after = data.get(endobj.saturating_add(6));
        // The last object of a file without a cross-reference table after it may end the file.
        if !is_eol(before) || (after.is_some() && !is_eol(after)) {
            note(&mut out.around_endobj);
        }
    }
    out
}

/// The white space and comments at `at` in `data`: how many bytes they take, and whether a
/// comment is among them.
fn space(data: &[u8], at: usize) -> (usize, bool) {
    let mut lx = Lexer::new(data, at);
    lx.skip_ws();
    // Past white space, a `%` can only start a comment.
    (lx.pos.saturating_sub(at), data.get(at..lx.pos).is_some_and(|gap| gap.contains(&b'%')))
}

/// Where the object value starting at `body` ends, past the white space and comments after it:
/// `endobj` when the object is written as ISO 32000-2 7.3.10 asks. `None` when the value can't be
/// read before `end`, the next object.
fn past_value(data: &[u8], body: usize, end: usize) -> Option<usize> {
    let mut lx = Lexer::new(data.get(..end)?, body);
    lx.object().ok()?;
    lx.skip_ws();
    Some(lx.pos)
}

/// Where the stream whose object body starts at `body` ends: past its `endstream` and the white
/// space and comments after it, as [`past_value`] reads past those after a value. The
/// `stream` keyword is the one after the dictionary as the lexer reads it, so `stream` in a string
/// of the dictionary isn't taken for it. `raw_len` is how long the document reads the data to be.
/// `None` when the dictionary, the `stream` keyword or `endstream` isn't there to read before
/// `end`, the next object.
fn past_stream(data: &[u8], body: usize, end: usize, raw_len: usize) -> Option<usize> {
    let mut lx = Lexer::new(data.get(..end)?, body);
    let Ok(Object::Dict(_)) = lx.object() else { return None };
    if !lx.eat_keyword(b"stream") {
        return None;
    }
    let start = lx.pos;
    let start = start.saturating_add(match data.get(start..start.saturating_add(2)) {
        Some(b"\r\n") => 2,
        Some([b'\r' | b'\n', ..]) => 1,
        _ => 0,
    });
    // The search starts past the data: data its /Length gives may end in the bytes `endstream`,
    // and data the reader recovered ends before the end-of-line marker ahead of `endstream`.
    let past = start.saturating_add(raw_len);
    let after = find(data, b"endstream", past.min(end), end)?.saturating_add(b"endstream".len());
    let mut lx = Lexer::new(data.get(..end)?, after);
    lx.skip_ws();
    Some(lx.pos)
}
