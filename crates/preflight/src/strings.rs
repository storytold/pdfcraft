//! PDF/A-2 and -3 6.1.6 (PDF/A-4 6.1.5), string objects: a hexadecimal string holds an even
//! number of non-white-space characters, each a hexadecimal digit, `0`–`9`, `A`–`F` or `a`–`f`
//! (ISO 32000-2 7.3.4.3). ISO 32000 lets a reader take a missing last digit as 0 and skip other
//! characters, so a parsed string no longer shows either: the strings are read as written.
//!
//! Two places hold them:
//!
//! - **The file.** The indirect objects the cross-reference sections place in the file, up to
//!   `stream` or `endobj`; the members of object streams, each from its offset to the next one's;
//!   and the trailers of cross-reference tables. Objects with an unsaved edit are left out, since
//!   the next save writes them afresh, and a document whose next save rewrites it in full is
//!   judged by that save, which writes every string with an even number of digits.
//! - **Content streams**, as `contents.rs` gives them. An inline image's dictionary is read and its
//!   data skipped, where the content parser ends it.

use std::collections::HashSet;
use std::ops::Range;

use pdfcraft_cos::{DecodeBudget, Document, ObjRef, Object, XrefEntry};

/// The decoded data, across the document's object and content streams, the scan reads.
const MAX_DECODED: usize = 256 << 20;
/// The members of one object stream that are read.
const MAX_MEMBERS: usize = 1 << 20;

/// What the hexadecimal strings in one place break.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Found {
    /// A string has an odd number of digits.
    pub(crate) odd: bool,
    /// A string holds a character that is neither a hexadecimal digit nor white space.
    pub(crate) not_hex: bool,
}

impl Found {
    fn all(self) -> bool {
        self.odd && self.not_hex
    }
}

/// What the hexadecimal strings of `doc` break, in the file and in content streams.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Hex {
    pub(crate) file: Found,
    pub(crate) content: Found,
}

/// Where a scan of some syntax stops.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Until {
    /// The end of an indirect object: `stream` or `endobj`.
    Object,
    /// The end of a trailer: `startxref`.
    Trailer,
    /// The end of the data.
    End,
}

fn is_ws(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// Reads the syntax `data` holds, from its start up to `until`, and records what its hexadecimal
/// strings break in `found`. `inline` gives the bytes of the inline image that starts at a `BI`
/// keyword (content only): its dictionary is read up to `ID`, and the rest skipped. Where no image
/// is found for a `BI` (the content parser reads none from a dictionary it can't parse), its
/// dictionary is read all the same, and the scan ends at its `ID`, since where its data ends isn't
/// known. A string that isn't closed ends the scan too. Returns how many bytes were read.
fn scan(data: &[u8], until: Until, found: &mut Found, inline: &mut dyn FnMut(usize) -> Option<usize>) -> usize {
    let mut i = 0;
    // Once a `BI` is passed: where its image ends, if the content parser found one there.
    let mut image_end: Option<Option<usize>> = None;
    while let Some(&b) = data.get(i) {
        if found.all() {
            return i;
        }
        match b {
            b'%' => {
                // A comment runs to the end of the line.
                i = data.get(i..).and_then(|rest| rest.iter().position(|&c| c == b'\r' || c == b'\n')).map_or(data.len(), |p| i.saturating_add(p));
            }
            b'(' => {
                let mut depth = 0usize;
                let mut closed = false;
                while let Some(&c) = data.get(i) {
                    i = i.saturating_add(1);
                    match c {
                        b'\\' => i = i.saturating_add(1),
                        b'(' => depth = depth.saturating_add(1),
                        b')' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                closed = true;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if !closed {
                    return i;
                }
            }
            b'<' if data.get(i.saturating_add(1)) == Some(&b'<') => i = i.saturating_add(2),
            b'<' => {
                let body = i.saturating_add(1);
                let Some(len) = data.get(body..).and_then(|rest| rest.iter().position(|&c| c == b'>')) else { return data.len() };
                let digits = data.get(body..body.saturating_add(len)).unwrap_or_default().iter().filter(|&&c| !is_ws(c));
                let (mut count, mut not_hex) = (0usize, false);
                for c in digits {
                    count = count.saturating_add(1);
                    not_hex |= !c.is_ascii_hexdigit();
                }
                found.not_hex |= not_hex;
                found.odd |= !not_hex && count % 2 == 1;
                i = body.saturating_add(len).saturating_add(1);
            }
            b'/' => {
                // A name, up to white space or a delimiter: never a keyword, whatever it spells.
                let len = data.get(i.saturating_add(1)..).and_then(|rest| rest.iter().position(|&c| is_ws(c) || is_delim(c)));
                i = len.map_or(data.len(), |len| i.saturating_add(1).saturating_add(len));
            }
            _ if is_ws(b) || is_delim(b) => i = i.saturating_add(1),
            _ => {
                // A keyword or a number: regular characters up to white space or a delimiter.
                let len = data.get(i..).and_then(|rest| rest.iter().position(|&c| is_ws(c) || is_delim(c))).unwrap_or(data.len().saturating_sub(i));
                let word = data.get(i..i.saturating_add(len)).unwrap_or_default();
                let start = i;
                i = i.saturating_add(len.max(1));
                match word {
                    b"stream" | b"endobj" if until == Until::Object => return i,
                    b"startxref" if until == Until::Trailer => return i,
                    b"BI" if image_end.is_none() => image_end = Some(inline(start)),
                    b"ID" => match image_end.take() {
                        Some(Some(end)) => i = end.max(i),
                        Some(None) => return i,
                        None => {}
                    },
                    _ => {}
                }
            }
        }
    }
    i
}

/// Reads one decoded content stream.
fn scan_content(data: &[u8], found: &mut Found) {
    let mut images: Option<Vec<Range<usize>>> = None;
    let mut inline = |at: usize| {
        // The content parser finds where each inline image ends; it is asked only when a `BI` is
        // met, and once per stream.
        let spans = images.get_or_insert_with(|| pdfcraft_content::ops(data).filter(|op| op.inline.is_some()).map(|op| op.span).collect());
        // The spans follow one another, in order: the one holding `at` is the last that starts at
        // or before it.
        let after = spans.partition_point(|span| span.start <= at);
        after.checked_sub(1).and_then(|k| spans.get(k)).filter(|span| at < span.end).map(|span| span.end)
    };
    scan(data, Until::End, found, &mut inline);
}

/// Whether `data` starts with an indirect object header, `num gen obj`.
fn object_header(data: &[u8]) -> bool {
    let digits = |d: &[u8]| d.iter().take_while(|c| c.is_ascii_digit()).count();
    let ws = |d: &[u8]| d.iter().take_while(|&&c| is_ws(c)).count();
    let mut at = digits(data);
    if at == 0 {
        return false;
    }
    let skip = |at: usize| data.get(at..).map_or(0, ws);
    let gap = skip(at);
    if gap == 0 {
        return false;
    }
    at = at.saturating_add(gap);
    let generation = data.get(at..).map_or(0, digits);
    if generation == 0 {
        return false;
    }
    at = at.saturating_add(generation);
    at = at.saturating_add(skip(at));
    data.get(at..).is_some_and(|rest| rest.starts_with(b"obj"))
}

/// The integers that start an object stream: its members' numbers and offsets, `n` pairs at most.
fn member_offsets(data: &[u8], n: usize) -> Vec<(u32, usize)> {
    let mut numbers = Vec::new();
    let mut i = 0;
    while numbers.len() < n.saturating_mul(2) {
        while data.get(i).is_some_and(|&c| is_ws(c)) {
            i = i.saturating_add(1);
        }
        let len = data.get(i..).map_or(0, |rest| rest.iter().take_while(|c| c.is_ascii_digit()).count());
        let Some(value) = data.get(i..i.saturating_add(len)).and_then(|d| std::str::from_utf8(d).ok()).and_then(|d| d.parse::<u64>().ok()) else {
            break;
        };
        numbers.push(value);
        i = i.saturating_add(len);
    }
    numbers.as_chunks::<2>().0.iter().filter_map(|[num, offset]| Some((u32::try_from(*num).ok()?, usize::try_from(*offset).ok()?))).collect()
}

/// What the hexadecimal strings of `doc` break, read as described in the module documentation.
/// `objs` are its objects and `content` the content streams that only a reference identifies.
pub(crate) fn check(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)], content: &HashSet<ObjRef>) -> Hex {
    let mut out = Hex::default();
    let mut budget = DecodeBudget::new(MAX_DECODED, 0);
    if !doc.full_save_required() && !doc.encryption_changed() {
        file(doc, &mut budget, &mut out.file);
    }
    let mut content_found = Found::default();
    crate::contents::each(doc, objs, content, MAX_DECODED, &mut |data| {
        scan_content(data, &mut content_found);
        !content_found.all()
    });
    out.content = content_found;
    out
}

/// The hexadecimal strings written in the file: the objects, the members of object streams and
/// the trailers.
fn file(doc: &Document, budget: &mut DecodeBudget, found: &mut Found) {
    let data = doc.bytes().as_slice();
    let mut none = |_: usize| None;
    // Each place is read once, and no more bytes are read than the file holds twice over, however
    // the cross-reference entries point into one another.
    let mut left = data.len().saturating_mul(2);
    let mut starts: HashSet<usize> = HashSet::new();
    let mut streams: HashSet<u32> = HashSet::new();
    for num in doc.object_numbers() {
        if found.all() || left == 0 {
            return;
        }
        if doc.is_edited(num) {
            continue;
        }
        match doc.xref_entry(num) {
            Some(XrefEntry::InFile { offset, .. }) => {
                let Ok(offset) = usize::try_from(offset) else { continue };
                // Offsets may count from a header that doesn't start the file.
                let at = [offset, offset.saturating_add(doc.header_offset())].into_iter().find(|&at| data.get(at..).is_some_and(object_header));
                let Some(at) = at.filter(|&at| starts.insert(at)) else { continue };
                let rest = data.get(at..).unwrap_or_default();
                let rest = rest.get(..rest.len().min(left)).unwrap_or_default();
                left = left.saturating_sub(scan(rest, Until::Object, found, &mut none));
            }
            Some(XrefEntry::InStream { stream, .. }) => {
                streams.insert(stream);
            }
            Some(XrefEntry::Free { .. }) | None => {}
        }
    }
    let mut streams: Vec<u32> = streams.into_iter().collect();
    streams.sort_unstable();
    for number in streams {
        if found.all() || budget.remaining() == 0 {
            return;
        }
        let r = ObjRef::new(number, doc.generation(number));
        let Object::Stream(s) = &*doc.get(r) else { continue };
        let (Some(n), Some(first)) = (s.dict.int(b"N"), s.dict.int(b"First")) else { continue };
        let (Ok(n), Ok(first)) = (usize::try_from(n), usize::try_from(first)) else { continue };
        let Ok(decoded) = budget.decode(s) else { continue };
        let mut members = member_offsets(&decoded, n.min(MAX_MEMBERS));
        members.sort_unstable_by_key(|&(_, offset)| offset);
        for (k, &(member, offset)) in members.iter().enumerate() {
            if doc.is_edited(member) {
                continue;
            }
            let start = first.saturating_add(offset);
            let end = members.get(k.saturating_add(1)).map_or(decoded.len(), |&(_, next)| first.saturating_add(next));
            if let Some(bytes) = decoded.get(start..end.max(start)) {
                scan(bytes, Until::End, found, &mut none);
            }
        }
    }
    for revision in doc.revisions().iter().filter(|r| !r.is_stream) {
        let shift = if revision.header_relative { doc.header_offset() } else { 0 };
        let at = usize::try_from(revision.xref_offset).ok().and_then(|o| o.checked_add(shift));
        if let Some(rest) = at.and_then(|at| data.get(at..)).filter(|rest| rest.starts_with(b"xref")) {
            scan(rest, Until::Trailer, found, &mut none);
        }
    }
}

/// Has the next save of `doc` write the file afresh when a hexadecimal string written in it has an
/// odd number of digits: the writer gives every string an even number. What it fixed, if anything.
pub(crate) fn rewrite(doc: &mut Document) -> Option<String> {
    if doc.full_save_required() || doc.encryption_changed() {
        return None;
    }
    let mut found = Found::default();
    file(doc, &mut DecodeBudget::new(MAX_DECODED, 0), &mut found);
    if !found.odd {
        return None;
    }
    doc.require_full_save();
    Some("Rewrote the hexadecimal strings with an odd number of digits".into())
}
