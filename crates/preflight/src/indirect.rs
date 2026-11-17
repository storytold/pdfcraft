//! PDF/A-2 and -3 6.1.9 (PDF/A-4 6.1.8): how an indirect object is written in the file, and 6.1.7.1
//! (PDF/A-4 6.1.6.1): how a stream's data is.
//!
//! ISO 19005-2/-3 6.1.9 and ISO 19005-4 6.1.8 narrow the syntax of ISO 32000-2 7.3.10:
//!
//! - the object number and the generation number are separated by one white-space character, and
//!   so are the generation number and the `obj` keyword;
//! - the object number is preceded by an end-of-line marker, and `obj` is followed by one;
//! - `endobj` is preceded by an end-of-line marker and followed by one.
//!
//! ISO 19005-2/-3 6.1.7.1 and ISO 19005-4 6.1.6.1 narrow those of 7.3.8.1 for a stream:
//!
//! - the `stream` keyword is followed by CR LF or by LF alone, not by CR alone or white space first;
//! - `endstream` is preceded by an end-of-line marker;
//! - `/Length` is the number of bytes from just past the marker after `stream` to the marker before
//!   `endstream`. With CR LF before `endstream`, a `/Length` that also counts the CR is taken as
//!   matching, as readers take it.
//!
//! The white-space characters are those of ISO 32000-2 Table 1, and an end-of-line marker is CR,
//! LF or CR LF (7.2.3).
//!
//! The objects read are those the cross-reference sections place in the file itself, at the
//! offset given there (counted from the file's start, or from a `%PDF-` header that doesn't start
//! it), or past white space at that offset. An object in an object stream has no such syntax, and
//! one not found at its offset is left out, since where it is written isn't known. A stream's
//! dictionary is read as written, and its `endstream` is where `/Length` puts it or, when `/Length`
//! is wrong, the first one past the data the document reads. An object's `endobj` is looked for
//! between its header and the next object the cross-reference sections place, after its value and
//! any comments after that (after `endstream` when it is a stream), so `endobj` inside a string,
//! name or comment isn't taken for it.

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
    /// 6.1.7.1: the `stream` keyword isn't followed by CR LF or by LF alone.
    pub(crate) after_stream: Option<u32>,
    /// 6.1.7.1: a stream's `/Length` doesn't give the bytes of its data.
    pub(crate) length: Option<u32>,
    /// 6.1.7.1: `endstream` isn't preceded by an end-of-line marker.
    pub(crate) before_endstream: Option<u32>,
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
        // An object with an unsaved edit is written again when the file is saved.
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
        // Where to look for `endobj`: past the stream data when the object is a stream, else past
        // its value and any comments after it, so `endobj` in a string, name or comment isn't
        // taken for it.
        let layout = match &*doc.get(ObjRef::new(num, doc.generation(num))) {
            Object::Stream(s) => stream_layout(doc, data, body, end, s.raw.len()),
            _ => None,
        };
        let from = match layout {
            Some(layout) => {
                if !layout.keyword_eol {
                    note(&mut out.after_stream);
                }
                if !layout.length_matches {
                    note(&mut out.length);
                }
                if !layout.eol_before_endstream {
                    note(&mut out.before_endstream);
                }
                layout.endstream.saturating_add(b"endstream".len())
            }
            None => past_value(data, body, end).unwrap_or(body),
        };
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

/// How a stream is laid out in the file around its data.
struct StreamLayout {
    /// The `stream` keyword is followed by CR LF or by LF alone.
    keyword_eol: bool,
    /// `/Length` gives the bytes from just past that end-of-line marker to the one before
    /// `endstream`.
    length_matches: bool,
    /// `endstream` is preceded by an end-of-line marker.
    eol_before_endstream: bool,
    /// Where `endstream` is.
    endstream: usize,
}

/// How the stream whose object body starts at `body` is laid out, read from the file's bytes up to
/// `end`, the next object. `raw_len` is how long the document reads its data to be, which finds
/// `endstream` when `/Length` doesn't. `None` when the dictionary, the `stream` keyword or
/// `endstream` isn't there to read.
fn stream_layout(doc: &Document, data: &[u8], body: usize, end: usize, raw_len: usize) -> Option<StreamLayout> {
    // The dictionary as written: the document's own copy has `/Length` set to what it read.
    let mut lx = Lexer::new(data.get(..end)?, body);
    let Ok(Object::Dict(dict)) = lx.object() else { return None };
    if !lx.eat_keyword(b"stream") {
        return None;
    }
    let keyword_end = lx.pos;
    // Spaces or tabs before the end-of-line marker are read past, as the document reads them, but
    // break the rule.
    let marker = keyword_end.saturating_add(data.get(keyword_end..end)?.iter().take_while(|&&b| matches!(b, b' ' | b'\t')).count());
    let spaced = marker > keyword_end;
    let (start, keyword_eol) = match (data.get(marker), data.get(marker.saturating_add(1))) {
        (Some(b'\r'), Some(b'\n')) => (marker.saturating_add(2), !spaced),
        (Some(b'\n'), _) => (marker.saturating_add(1), !spaced),
        (Some(b'\r'), _) => (marker.saturating_add(1), false),
        _ => (keyword_end, false),
    };
    let declared = match dict.get(b"Length") {
        Some(Object::Int(n)) => Some(*n),
        Some(Object::Ref(r)) => match &*doc.get(*r) {
            Object::Int(n) => Some(*n),
            _ => None,
        },
        _ => None,
    };
    // Where `/Length` puts the end of the data, when an end-of-line marker and `endstream`, or
    // `endstream` alone, follow it there; and how long that marker is.
    let by_length = declared.and_then(|l| usize::try_from(l).ok()).and_then(|l| start.checked_add(l)).filter(|&p| p <= end).and_then(|p| {
        let rest = data.get(p..end)?;
        let eol = match rest {
            [b'\r', b'\n', ..] => 2,
            [b'\r' | b'\n', ..] => 1,
            _ => 0,
        };
        let e = p.saturating_add(eol);
        endstream_at(data, e).then_some((e, eol))
    });
    let (endstream, length_matches) = match by_length {
        // `endstream` right at the end of the data: `/Length` counted the marker before it, if
        // there is one, unless the data is empty and that marker is the one after `stream`.
        Some((e, 0)) => (e, e == start || !is_eol(e.checked_sub(1).and_then(|p| data.get(p)))),
        Some((e, _)) => (e, true),
        None => {
            // The stream data never holds `endstream` itself, so the search starts just inside it.
            let past = start.saturating_add(raw_len).saturating_sub(b"endstream".len()).max(start);
            (find_endstream(data, past.min(end), end)?, false)
        }
    };
    let eol_before_endstream = is_eol(endstream.checked_sub(1).and_then(|p| data.get(p)));
    Some(StreamLayout { keyword_eol, length_matches, eol_before_endstream, endstream })
}

/// Whether a whole `endstream` token starts at `at`: `endstreamer` is stream data (7.2.3).
fn endstream_at(data: &[u8], at: usize) -> bool {
    data.get(at..).is_some_and(|d| d.starts_with(b"endstream"))
        && !data.get(at.saturating_add(b"endstream".len())).is_some_and(|&b| !is_whitespace(b) && !b"()<>[]{}/%".contains(&b))
}

/// The first whole `endstream` token in `from..to`.
fn find_endstream(data: &[u8], from: usize, to: usize) -> Option<usize> {
    let mut at = from;
    while let Some(k) = find(data, b"endstream", at, to) {
        if endstream_at(data, k) {
            return Some(k);
        }
        at = k.saturating_add(1);
    }
    None
}
