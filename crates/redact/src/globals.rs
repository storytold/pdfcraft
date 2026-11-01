//! A `/JBIG2Globals` stream is a dictionary of symbols that JBIG2 images can share, and scanners
//! and `jbig2enc -s` share one across a scan's pages. It holds the shape of every distinct mark of
//! those pages, including a mark found only under a redaction, so taking the image out isn't enough:
//! while something else uses the dictionary, a full save keeps it. [`unshare`] stores what uses it
//! again without it.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use pdfcraft_cos::{Document, MAX_JBIG2_GLOBALS, Object, Stream, reachable};

/// The references followed from `/JBIG2Globals` towards a stream, so that a ring of them ends.
const MAX_HOPS: usize = 32;

/// The objects a full save would keep, by number: a reference reads the object of its number,
/// whatever generation it gives.
pub(crate) fn kept(doc: &Document) -> HashSet<u32> {
    reachable(doc).into_iter().map(|r| r.num).collect()
}

/// `o`, or what references lead to from it. A ring of them ends as a reference, which is no dictionary.
fn followed(doc: &Document, o: &Object) -> Arc<Object> {
    let mut at = doc.resolve(o);
    for _ in 0..MAX_HOPS {
        let next = match &*at {
            Object::Ref(next) => *next,
            _ => break,
        };
        at = doc.get(next);
    }
    at
}

/// Every object `s` names as `/JBIG2Globals`, and each reference on the way to one: in any entry of
/// its parameters, whichever filter that entry belongs to and however the filters are given. A
/// reader may use less of this, but leaving a dictionary behind is the failure that matters here.
fn named(doc: &Document, s: &Stream) -> Vec<u32> {
    let mut found = Vec::new();
    let mut read = |entry: &Object| {
        let held = followed(doc, entry);
        let Some(mut at) = held.as_dict().and_then(|d| d.get(b"JBIG2Globals")).and_then(Object::as_ref) else { return };
        for _ in 0..=MAX_HOPS {
            found.push(at.num);
            match &*doc.get(at) {
                Object::Ref(next) => at = *next,
                _ => return,
            }
        }
    };
    for key in [b"DecodeParms".as_slice(), b"DP"] {
        let Some(parms) = s.dict.get(key).map(|p| followed(doc, p)) else { continue };
        match &*parms {
            Object::Array(entries) => entries.iter().for_each(&mut read),
            other => read(other),
        }
    }
    found
}

/// `v`, or what it refers to, inside an array too.
fn direct(doc: &Document, v: &Object) -> Object {
    match &*doc.resolve(v) {
        Object::Array(a) => Object::Array(a.iter().map(|e| (*doc.resolve(e)).clone()).collect()),
        other => other.clone(),
    }
}

/// `s` with what `decoded_image` and `decoded_within` read as given made direct: its size and filters.
fn readable(doc: &Document, s: &Stream) -> Stream {
    let mut readable = s.clone();
    for key in [b"Width".as_slice(), b"Height", b"Filter"] {
        if let Some(v) = s.dict.get(key) {
            readable.dict.set(key.to_vec(), direct(doc, v));
        }
    }
    readable
}

/// What dictionary `num` holds, to know a copy of it by: a file put together from pages split apart
/// has one per page, each byte for byte the others.
fn content(doc: &Document, num: u32) -> Option<u64> {
    let object = doc.try_get(num).ok()?;
    let Object::Stream(s) = &*object else { return None };
    let readable = readable(doc, s);
    let data = doc.with_direct_parms(&readable).decoded_within(MAX_JBIG2_GLOBALS).unwrap_or_else(|_| s.raw.to_vec());
    let mut hash = DefaultHasher::new();
    data.hash(&mut hash);
    Some(hash.finish())
}

/// JBIG2 stream `s` stored alone, as a cleared copy is: what it shows unchanged and no dictionary
/// referred to. `None` for a stream that isn't JBIG2, whose decoder took no dictionary and whose
/// samples another filter may give otherwise; for one past the size of an image redaction clears,
/// or known to have more than one component, which JBIG2 is drawn as one of whatever the dictionary
/// says; and for one that can't be decoded.
fn without_dictionary(doc: &Document, s: &Stream) -> Option<Stream> {
    let readable = readable(doc, s);
    let jbig2 = match readable.dict.get(b"Filter") {
        Some(Object::Array(a)) => a.iter().any(|f| f.as_name() == Some(b"JBIG2Decode".as_slice())),
        Some(f) => f.as_name() == Some(b"JBIG2Decode".as_slice()),
        None => false,
    };
    if !jbig2 || (s.dict.contains(b"ColorSpace") && crate::image::components(doc, s).is_some_and(|n| n > 1)) {
        return None;
    }
    let side = |key: &[u8]| readable.dict.int(key).and_then(|v| usize::try_from(v).ok());
    let (w, h) = (side(b"Width")?, side(b"Height")?);
    if (w as u64).saturating_mul(h as u64) > crate::image::MAX_PIXELS {
        return None;
    }
    // As for `clear`: the filters before the JBIG2 decoder may give up to three bytes for each one
    // of the raster and a byte a row.
    let allowance = w.div_ceil(8).checked_mul(h)?.checked_add(h)?.checked_mul(3)?.checked_add(4096)?;
    let data = doc.decoded_image_within(&readable, allowance).ok()?;
    let mut dict = readable.dict.clone();
    for key in [b"Length".as_slice(), b"DecodeParms", b"DP"] {
        dict.remove(key);
    }
    dict.set(b"BitsPerComponent".to_vec(), Object::Int(1));
    Some(crate::image::smallest(dict, &data, w, h, true, true))
}

/// Takes the `/JBIG2Globals` dictionaries of what left the document since `before` ([`kept`]) out of
/// the file: everything a full save would keep that names one, or a copy of one, is stored again
/// without it, in place, so what refers to that stream stays as it is. Gives the number stored
/// again. A copy is found byte for byte; another dictionary that happens to hold the same shapes isn't.
///
/// What left, not what a run says it removed: an image goes with a stamp, a widget, a Type 3 glyph,
/// an `/Alternates` entry or a form that can't be read, none of which it was drawn through directly.
/// An image another page still draws hasn't left: it keeps its pixels, so its dictionary keeps nothing
/// more.
///
/// Errors: [`RedactError::SharedJbig2Globals`](crate::RedactError::SharedJbig2Globals) when a dictionary would
/// still be kept, as what names it can't be stored without it or something else refers to it. Such
/// an image may still be drawn, on a page no mark reached, so it isn't removed, and `doc` is part
/// changed by then: a caller works on a copy it can drop.
pub(crate) fn unshare(doc: &mut Document, before: &HashSet<u32>) -> Result<usize, crate::RedactError> {
    let now = kept(doc);
    let mut dictionaries: HashSet<u32> = HashSet::new();
    for &num in before.difference(&now) {
        if let Ok(object) = doc.try_get(num)
            && let Object::Stream(s) = &*object
        {
            dictionaries.extend(named(doc, s));
        }
    }
    if dictionaries.is_empty() {
        return Ok(0);
    }
    let contents: HashSet<u64> = dictionaries.iter().filter_map(|n| content(doc, *n)).collect();
    let mut read: HashMap<u32, Option<u64>> = HashMap::new();
    let mut users = Vec::new();
    for (r, object) in doc.scan_objects() {
        let Object::Stream(s) = &*object else { continue };
        if !now.contains(&r.num) {
            continue;
        }
        let names = named(doc, s);
        let copy =
            |n: &u32, read: &mut HashMap<u32, Option<u64>>| read.entry(*n).or_insert_with(|| content(doc, *n)).is_some_and(|c| contents.contains(&c));
        if names.iter().any(|n| dictionaries.contains(n) || copy(n, &mut read)) {
            dictionaries.extend(names);
            users.push(r);
        }
    }
    let mut stuck = 0;
    for &r in &users {
        let object = doc.get(r);
        let Object::Stream(s) = &*object else { continue };
        match without_dictionary(doc, s) {
            Some(stored) => doc.set(r, Object::Stream(stored)),
            None => stuck += 1,
        }
    }
    let after = kept(doc);
    if dictionaries.iter().any(|n| after.contains(n)) {
        return Err(crate::RedactError::SharedJbig2Globals(stuck.max(1)));
    }
    Ok(users.len())
}
