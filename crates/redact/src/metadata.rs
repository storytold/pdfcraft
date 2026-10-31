//! Metadata that repeats redacted text (#2448).
//!
//! [`crate::apply`] removes the glyphs under the marks, but the same words are often in the
//! document information dictionary (`/Title`, `/Subject`, `/Keywords`…) and the XMP metadata
//! that mirrors it. Once the page no longer shows them, the metadata must not keep them either.
//!
//! An `/Info` entry whose text contains a redacted term is removed, and so is one whose whole
//! value is part of a removed run (an author "J. Li" redacted from a sentence, whose words are too
//! short to be terms; the reverse check #2598 also proposed). An XMP stream whose text
//! (element content, attribute values other than namespace declarations, comments and
//! processing instructions, entities decoded) contains one, or has a value that is part of a
//! removed run, is removed whole: editing the XML in place could leave it malformed or keep the
//! term in another spelling. A PDF/A identification in that packet goes with it.
//!
//! Fail-closed: XMP that isn't UTF-8 or UTF-16 goes, text too large to search against every term
//! counts as matching, and when the removed text itself was cut short (too many runs or terms,
//! or a run too long to keep) every `/Info` string and every XMP stream goes, since what was
//! never collected can't be searched for. Terms are matched without regard to case or spacing.

use std::collections::HashSet;

use pdfcraft_cos::{Document, ObjRef, Object};

/// Terms longer than this are cut (matching still finds the prefix).
const MAX_TERM_CHARS: usize = 4096;
const MAX_TERMS: usize = 20_000;
/// XMP packets larger than this, decoded, are removed without reading them.
const MAX_XMP_BYTES: usize = 16 * 1024 * 1024;
/// The searching (text length times terms, or value length times removed text) one scrub may
/// do in all; past it every entry and packet counts as matching (fail-closed), so a page of
/// removed text against a large packet, or against very many small values, stays quick.
const MAX_WORK: usize = 1 << 28;

/// Lower-case `s` with every run of whitespace made one space, trimmed.
fn normalize(s: &str) -> String {
    s.split_whitespace().flat_map(|w| w.chars().flat_map(char::to_lowercase).chain(std::iter::once(' '))).collect::<String>().trim_end().to_string()
}

fn significant(s: &str, min: usize) -> bool {
    s.chars().filter(|c| c.is_alphanumeric()).count() >= min
}

/// The terms to look for: each removed run (at least two letters or digits), and each of its
/// words with at least three, so "Smith, John" in `/Author` is caught when "John Smith" was
/// redacted. Shorter pieces would match nearly any text. `None` when a run is longer than
/// [`MAX_TERM_CHARS`] or there are more than [`MAX_TERMS`] terms: the caller then can't search
/// for all of it and must clear the metadata instead.
pub(crate) fn terms(removed: &[String]) -> Option<Vec<String>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for run in removed {
        let run = normalize(run);
        if run.chars().nth(MAX_TERM_CHARS).is_some() {
            return None;
        }
        let words = run.split(|c: char| !c.is_alphanumeric()).filter(|w| significant(w, 3)).map(str::to_string).collect::<Vec<_>>();
        for t in std::iter::once(run.clone()).filter(|r| significant(r, 2)).chain(words) {
            if seen.insert(t.clone()) {
                if out.len() >= MAX_TERMS {
                    return None;
                }
                out.push(t);
            }
        }
    }
    Some(out)
}

/// The searching one scrub may do, across every `/Info` entry and XMP value: past it, each
/// further question is answered "yes" (fail-closed), so a packet of many small values can't add
/// up to more than one large one.
struct Work(std::cell::Cell<usize>);

impl Work {
    fn new(limit: usize) -> Self {
        Self(std::cell::Cell::new(limit))
    }

    /// Take `cost` from what is left; `false` (and nothing left) when it doesn't fit.
    fn spend(&self, cost: usize) -> bool {
        match self.0.get().checked_sub(cost) {
            Some(left) => {
                self.0.set(left);
                true
            }
            None => {
                self.0.set(0);
                false
            }
        }
    }
}

fn mentions(text: &str, terms: &[String], work: &Work) -> bool {
    let text = normalize(text);
    !work.spend(text.len().saturating_mul(terms.len())) || terms.iter().any(|t| text.contains(t.as_str()))
}

/// Is `value` itself part of one of the removed `runs` (normalized), as whole words? It needs
/// three letters or digits, so an entry such as "by" doesn't go with every sentence that
/// contains it, and word boundaries, so an XMP value "True" doesn't go with "untrue". Too much
/// text to search counts as part (fail-closed).
fn within(value: &str, runs: &[String], work: &Work) -> bool {
    let value = normalize(value);
    if !significant(&value, 3) {
        return false;
    }
    let total = runs.iter().map(String::len).fold(0usize, usize::saturating_add);
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let whole = |r: &String| {
        r.match_indices(value.as_str()).any(|(at, m)| {
            let before = r.get(..at).and_then(|s| s.chars().next_back());
            let after = r.get(at.saturating_add(m.len())..).and_then(|s| s.chars().next());
            !(word(before) && value.starts_with(char::is_alphanumeric)) && !(word(after) && value.ends_with(char::is_alphanumeric))
        })
    };
    !work.spend(value.len().saturating_mul(total)) || runs.iter().any(whole)
}

/// Decode the five predefined XML entities and character references; anything else stays.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or(""));
        let tail = rest.get(at..).unwrap_or("");
        // A named entity ends within 12 bytes; looking further made a text of bare `&`s
        // quadratic. A character reference may carry any number of leading zeros, so its digits
        // are read to their end: each digit belongs to one `&` only, which keeps it linear.
        let end = match tail.strip_prefix("&#") {
            Some(digits) => {
                let hex = digits.starts_with(['x', 'X']);
                let from = 2 + usize::from(hex);
                let run = tail.get(from..).unwrap_or("").bytes().take_while(|b| if hex { b.is_ascii_hexdigit() } else { b.is_ascii_digit() }).count();
                let e = from + run;
                (tail.as_bytes().get(e) == Some(&b';')).then_some(e)
            }
            None => tail.char_indices().take(13).find(|&(_, c)| c == ';').map(|(e, _)| e),
        };
        let entity = end.and_then(|e| tail.get(1..e));
        let decoded = entity.and_then(|e| match e {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                let n = match e.strip_prefix("#x").or_else(|| e.strip_prefix("#X")) {
                    // Leading zeros dropped, so they can't overflow the number; `&#0;` isn't a
                    // character XML allows, and stays as written.
                    Some(h) => u32::from_str_radix(h.trim_start_matches('0'), 16).ok(),
                    None => e.strip_prefix('#').and_then(|d| d.trim_start_matches('0').parse().ok()),
                };
                n.and_then(char::from_u32)
            }
        });
        match (decoded, end) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = tail.get(e + 1..).unwrap_or("");
            }
            _ => {
                out.push('&');
                rest = tail.get(1..).unwrap_or("");
            }
        }
    }
    out.push_str(rest);
    out
}

/// The text values an XML document carries, entities decoded: element content, attribute values
/// (XMP stores simple properties as attributes), CDATA, and the text of comments and processing
/// instructions other than the `xpacket` wrapper, which a reader can still see. Names and markup are left out, so a term can't
/// match a tag such as `dc:title`, and so are namespace declarations (`xmlns`), whose URIs
/// would match words such as "adobe" or "application" in every packet.
pub(crate) fn xml_values(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |s: String| {
        if !s.trim().is_empty() {
            out.push(s);
        }
    };
    let mut rest = xml;
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            push(unescape(rest));
            break;
        };
        push(unescape(rest.get(..open).unwrap_or("")));
        let tag = rest.get(open..).unwrap_or("");
        let delimited = [("<![CDATA[", "]]>"), ("<!--", "-->"), ("<?", "?>")].into_iter().find(|(start, _)| tag.starts_with(start));
        if let Some((start, stop)) = delimited {
            let body = tag.get(start.len()..).unwrap_or("");
            let end = body.find(stop).unwrap_or(body.len());
            let text = body.get(..end).unwrap_or("");
            // The packet wrapper's fixed `begin`/`id`/`end` would match words such as "end".
            if !(start == "<?" && text.split_whitespace().next() == Some("xpacket")) {
                push(text.to_string());
            }
            rest = body.get(end.saturating_add(stop.len())..).unwrap_or("");
            continue;
        }
        // Attribute values inside the tag, up to its closing `>` (which can't be inside quotes).
        let mut chars = tag.char_indices().skip(1);
        let mut end = tag.len();
        // The attribute name being read, and the last one read before whitespace: XML allows
        // whitespace around `=` (`xmlns:x = "…"`).
        let mut name = String::new();
        let mut last = String::new();
        while let Some((i, c)) = chars.next() {
            match c {
                '>' => {
                    end = i + 1;
                    break;
                }
                '"' | '\'' => {
                    let mut value = String::new();
                    for (_, d) in chars.by_ref() {
                        if d == c {
                            break;
                        }
                        value.push(d);
                    }
                    let attr = if name.is_empty() { &last } else { &name };
                    let declaration = attr == "xmlns" || attr.starts_with("xmlns:");
                    if !declaration {
                        push(unescape(&value));
                    }
                    name.clear();
                    last.clear();
                }
                '=' => {}
                '/' => {
                    name.clear();
                    last.clear();
                }
                c if c.is_whitespace() => {
                    if !name.is_empty() {
                        last = std::mem::take(&mut name);
                    }
                }
                c => name.push(c),
            }
        }
        rest = tag.get(end..).unwrap_or("");
    }
    out
}

/// Decode an XMP packet: UTF-8 (with or without a byte-order mark), or UTF-16 told by its mark
/// or, as a packet usually begins `<?xpacket` with the mark inside the attribute, by the zero
/// byte of its first character. `None` for anything else or any invalid sequence, so text that
/// can't be read faithfully is never searched lossily (the caller removes it).
pub(crate) fn decode_xmp(bytes: &[u8]) -> Option<String> {
    let utf16 = |body: &[u8], be: bool| -> Option<String> {
        let (units, rest) = body.as_chunks::<2>();
        if !rest.is_empty() {
            return None;
        }
        char::decode_utf16(units.iter().map(|&p| if be { u16::from_be_bytes(p) } else { u16::from_le_bytes(p) })).collect::<Result<String, _>>().ok()
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => std::str::from_utf8(rest).ok().map(str::to_string),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0, a, ..] if *a != 0 => utf16(bytes, true),
        [a, 0, ..] if *a != 0 => utf16(bytes, false),
        _ => std::str::from_utf8(bytes).ok().map(str::to_string),
    }
}

/// The text values of an XMP stream, or `None` when it can't be read.
fn xmp_values(doc: &Document, o: &Object) -> Option<Vec<String>> {
    let Object::Stream(s) = &*doc.resolve(o) else { return None };
    // Decoding stops at the cap, so a small stream that inflates hugely costs no more memory.
    let bytes = s.decoded_within(MAX_XMP_BYTES).ok()?;
    decode_xmp(&bytes).map(|t| xml_values(&t))
}

/// Remove the metadata that still holds `removed` text: matching `/Info` entries, and the
/// catalog's and pages' XMP streams that mention it. `complete` is false when the collection of
/// removed text was cut short; then, as when the terms themselves exceed their limits, every
/// `/Info` string and every XMP stream goes. Returns how many entries and streams went.
pub(crate) fn scrub(doc: &mut Document, removed: &[String], complete: bool) -> Result<usize, pdfcraft_cos::CosError> {
    scrub_within(doc, removed, complete, MAX_WORK)
}

/// [`scrub`] with the searching it may do given, so tests can run out of it.
pub(crate) fn scrub_within(doc: &mut Document, removed: &[String], complete: bool, limit: usize) -> Result<usize, pdfcraft_cos::CosError> {
    // `None` matches everything (fail-closed).
    let search = match terms(removed).filter(|_| complete) {
        Some(t) if t.is_empty() => return Ok(0),
        Some(t) => {
            // The same run removed many times is searched for once.
            let mut seen = HashSet::new();
            let runs: Vec<String> = removed.iter().map(|r| normalize(r)).filter(|r| seen.insert(r.clone())).collect();
            Some((t, runs))
        }
        None => None,
    };
    let work = Work::new(limit);
    let hit = |text: &str| search.as_ref().is_none_or(|(terms, runs)| mentions(text, terms, &work) || within(text, runs, &work));
    let mut n = 0;

    // The information dictionary, direct in the trailer or an object of its own.
    let info_obj = doc.trailer().get(b"Info").cloned();
    if let Some(info) = info_obj.as_ref().and_then(|i| doc.resolve(i).as_dict().cloned()) {
        let doomed: Vec<Vec<u8>> =
            info.iter().filter(|(_, v)| doc.resolve(v).as_string().is_some_and(|s| hit(&s.to_text()))).map(|(k, _)| k.to_vec()).collect();
        if !doomed.is_empty() {
            n += doomed.len();
            let mut kept = info.clone();
            for k in &doomed {
                kept.remove(k);
            }
            match info_obj.as_ref().and_then(Object::as_ref) {
                Some(r) => doc.update_dict(r, |d| *d = kept)?,
                None => doc.trailer_mut().set(b"Info".to_vec(), Object::Dict(kept)),
            }
        }
    }

    // XMP on the catalog and on every page.
    let mut holders: Vec<ObjRef> = doc.root().into_iter().collect();
    holders.extend(pdfcraft_model::pages(doc).iter().map(|p| p.obj));
    for holder in holders {
        let Some(xmp) = doc.get(holder).as_dict().and_then(|d| d.get(b"Metadata").cloned()) else { continue };
        let doomed = match xmp_values(doc, &xmp) {
            None => true,
            Some(values) => {
                let words = |(terms, _): &(Vec<String>, Vec<String>)| mentions(&values.join(" "), terms, &work);
                let part = |(_, runs): &(Vec<String>, Vec<String>)| values.iter().any(|v| within(v, runs, &work));
                search.as_ref().is_none_or(|s| words(s) || part(s))
            }
        };
        if doomed {
            doc.update_dict(holder, |d| {
                d.remove(b"Metadata");
            })?;
            n += 1;
        }
    }
    Ok(n)
}
