//! A `/JBIG2Globals` stream is a dictionary of symbols that JBIG2 images can share, and scanners
//! and `jbig2enc -s` share one across a scan's pages. It holds the shape of every distinct mark of
//! those pages, including a mark found only under a redaction, so taking the image out isn't enough:
//! while something else uses the dictionary, a full save keeps it. [`unshare`] stores what uses it
//! again without it.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use pdfcraft_cos::{DECODE_PARMS_KEYS, Dict, Document, MAX_JBIG2_GLOBALS, Object, Stream, reachable};

/// The references followed from `/JBIG2Globals` towards a stream, so that a ring of them ends.
const MAX_HOPS: usize = 32;

/// The objects a full save would keep, by number: a reference reads the object of its number,
/// whatever generation it gives.
pub(crate) fn kept(doc: &Document) -> HashSet<u32> {
    reachable(doc).into_iter().map(|r| r.num).collect()
}

/// `o`, or what references lead to from it. A ring of them ends as a reference, which is no dictionary.
pub(crate) fn followed(doc: &Document, o: &Object) -> Arc<Object> {
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

/// What a parameters value names as `/JBIG2Globals`, and the key it is remembered by: a value the
/// file refers to, by its object number and whether it is read as a list of entries.
pub(crate) type Listed = (Rc<[u32]>, Option<(u32, bool)>);

/// Every object a stream names as `/JBIG2Globals`, and each reference on the way to one: in any entry
/// of its parameters, whichever filter that entry belongs to and however the filters are given. A
/// reader may use less of this, but leaving a dictionary behind is the failure that matters here.
///
/// What a value the file refers to names is read once a run, so streams that share one, by
/// reference or through an entry, cost it once however many there are.
#[derive(Default)]
pub(crate) struct Names {
    memo: HashMap<(u32, bool), Rc<[u32]>>,
    /// How many values were read rather than remembered.
    pub(crate) read: usize,
}

impl Names {
    /// What `s` names, one list for each key that gives it parameters.
    pub(crate) fn of(&mut self, doc: &Document, s: &Stream) -> Vec<Listed> {
        // `/FDecodeParms` too: those of data in another file, which a reader may draw instead.
        [b"DecodeParms".as_slice(), b"DP", b"FDecodeParms"].into_iter().filter_map(|key| s.dict.get(key)).map(|v| self.value(doc, v, true)).collect()
    }

    /// What `v` names: one entry, or (`list`) an array of them or one.
    fn value(&mut self, doc: &Document, v: &Object, list: bool) -> Listed {
        let key = v.as_ref().map(|r| (r.num, list));
        if let Some(hit) = key.and_then(|k| self.memo.get(&k)) {
            return (hit.clone(), key);
        }
        self.read += 1;
        let names: Rc<[u32]> = match (&*followed(doc, v), list) {
            (Object::Array(entries), true) => {
                let mut all: Vec<u32> = entries.iter().flat_map(|e| self.value(doc, e, false).0.to_vec()).collect();
                all.sort_unstable();
                all.dedup();
                all.into()
            }
            (entry, _) => entry_names(doc, entry).into(),
        };
        if let Some(k) = key {
            self.memo.insert(k, names.clone());
        }
        (names, key)
    }
}

/// What one parameters entry names as `/JBIG2Globals`, and each reference on the way to it.
fn entry_names(doc: &Document, entry: &Object) -> Vec<u32> {
    let mut found = Vec::new();
    let Some(mut at) = entry.as_dict().and_then(|d| d.get(b"JBIG2Globals")).and_then(Object::as_ref) else { return found };
    for _ in 0..=MAX_HOPS {
        found.push(at.num);
        match &*doc.get(at) {
            Object::Ref(next) => at = *next,
            _ => break,
        }
    }
    found
}

/// A `/Filter` through any references, as a decoder reads it: a name, or an array of names, each
/// entry through references too. Anything else is null, a chain longer than any decoder takes
/// (`MAX_CHAIN`, and a `/Crypt`) included, so nothing large is copied however often the file refers
/// to it, and a stream that names it is read as one with no filter.
fn direct_filter(doc: &Document, v: &Object) -> Object {
    let name = |o: &Object| if matches!(o, Object::Name(_)) { o.clone() } else { Object::Null };
    match &*followed(doc, v) {
        Object::Array(a) if a.len() > pdfcraft_filters::MAX_CHAIN + 1 => Object::Null,
        Object::Array(a) => Object::Array(a.iter().map(|e| name(&followed(doc, e))).collect()),
        other => name(other),
    }
}

/// A size through any references: a number, or null.
fn direct_number(doc: &Document, v: &Object) -> Object {
    match &*followed(doc, v) {
        number @ (Object::Int(_) | Object::Real(_)) => number.clone(),
        _ => Object::Null,
    }
}

/// `/DecodeParms` as a reader takes them, through any references, for `filters` filters: of each
/// entry, the numbers and flags the decoders read ([`DECODE_PARMS_KEYS`]), and `/JBIG2Globals` when it
/// is a reference. An entry that isn't a dictionary is null. As `Document::with_direct_parms`, nothing large
/// is copied however often the file refers to it.
fn direct_parms(doc: &Document, v: &Object, filters: usize) -> Object {
    let entry = |e: &Object| match &*followed(doc, e) {
        Object::Dict(d) => {
            let mut made = Dict::new();
            for key in DECODE_PARMS_KEYS {
                let Some(value) = d.get(key) else { continue };
                let value = followed(doc, value);
                if matches!(*value, Object::Int(_) | Object::Real(_) | Object::Bool(_)) {
                    made.set(key.to_vec(), (*value).clone());
                }
            }
            // As a reference only, as `Document::with_direct_parms` keeps it: a direct value is no
            // stream, so no decoder reads it, and it may be large.
            if let Some(globals @ Object::Ref(_)) = d.get(b"JBIG2Globals") {
                made.set(b"JBIG2Globals".to_vec(), globals.clone());
            }
            Object::Dict(made)
        }
        _ => Object::Null,
    };
    match &*followed(doc, v) {
        Object::Array(a) => Object::Array(a.iter().take(filters).map(entry).collect()),
        other => entry(other),
    }
}

/// `s` with what `decoded_image` and `decoded_within` read as given made direct: its size, filters
/// and their parameters.
pub(crate) fn readable(doc: &Document, s: &Stream) -> Stream {
    let mut readable = s.clone();
    for key in [b"Width".as_slice(), b"Height"] {
        if let Some(v) = s.dict.get(key) {
            readable.dict.set(key.to_vec(), direct_number(doc, v));
        }
    }
    if let Some(v) = s.dict.get(b"Filter") {
        readable.dict.set(b"Filter".to_vec(), direct_filter(doc, v));
    }
    if let Some(v) = s.dict.get(b"DecodeParms") {
        let filters = match readable.dict.get(b"Filter") {
            Some(Object::Array(a)) => a.len(),
            Some(Object::Name(_)) => 1,
            _ => 0,
        };
        readable.dict.set(b"DecodeParms".to_vec(), direct_parms(doc, v, filters));
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
/// says; for one whose data is in another file (`/F`), which readers draw instead of what is here;
/// and for one that can't be decoded.
fn without_dictionary(doc: &Document, s: &Stream) -> Option<Stream> {
    if crate::image::data_elsewhere(doc, s) {
        return None;
    }
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
    // `/FFilter` and `/FDecodeParms` too: with no data in another file they mean nothing, but
    // `named` reads them.
    for key in [b"Length".as_slice(), b"DecodeParms", b"DP", b"FFilter", b"FDecodeParms"] {
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
/// still be kept, as what names it can't be stored without it (counted) or something else refers to
/// it (0 when that is all). Such an image may still be drawn, on a page no mark reached, so it isn't removed, and `doc` is part
/// changed by then: a caller works on a copy it can drop.
pub(crate) fn unshare(doc: &mut Document, before: &HashSet<u32>) -> Result<usize, crate::RedactError> {
    let now = kept(doc);
    let mut names = Names::default();
    let mut dictionaries: HashSet<u32> = HashSet::new();
    // A list the file refers to is added once, however many streams share it.
    let mut added: HashSet<(u32, bool)> = HashSet::new();
    for &num in before.difference(&now) {
        if let Ok(object) = doc.try_get(num)
            && let Object::Stream(s) = &*object
        {
            for (list, key) in names.of(doc, s) {
                if key.is_none_or(|k| added.insert(k)) {
                    dictionaries.extend(list.iter());
                }
            }
        }
    }
    if dictionaries.is_empty() {
        return Ok(0);
    }
    let contents: HashSet<u64> = dictionaries.iter().filter_map(|n| content(doc, *n)).collect();
    let mut read: HashMap<u32, Option<u64>> = HashMap::new();
    // Whether a name matches doesn't depend on order: it is a dictionary that left, or a copy of one
    // (what is added below is only ever such a copy). So what a shared list matches is found once.
    let left = dictionaries.clone();
    let mut matches: HashMap<(u32, bool), Rc<[u32]>> = HashMap::new();
    added.clear();
    let mut users = Vec::new();
    for (r, object) in doc.scan_objects() {
        let Object::Stream(s) = &*object else { continue };
        if !now.contains(&r.num) {
            continue;
        }
        let copy =
            |n: &u32, read: &mut HashMap<u32, Option<u64>>| read.entry(*n).or_insert_with(|| content(doc, *n)).is_some_and(|c| contents.contains(&c));
        // Only what matched has to leave, a copy found here included. Another dictionary this also
        // names was shared with nothing that left: what else uses it may keep it, whichever comes
        // first in the file.
        let mut uses = false;
        for (list, key) in names.of(doc, s) {
            let matched = match key.and_then(|k| matches.get(&k)) {
                Some(hit) => hit.clone(),
                None => {
                    let found: Rc<[u32]> = list.iter().copied().filter(|n| left.contains(n) || copy(n, &mut read)).collect();
                    if let Some(k) = key {
                        matches.insert(k, found.clone());
                    }
                    found
                }
            };
            if !matched.is_empty() {
                uses = true;
                if key.is_none_or(|k| added.insert(k)) {
                    dictionaries.extend(matched.iter());
                }
            }
        }
        if uses {
            users.push(r);
        }
    }
    let (mut stored_again, mut stuck) = (0, 0);
    for &r in &users {
        let object = doc.get(r);
        let Object::Stream(s) = &*object else { continue };
        match without_dictionary(doc, s) {
            Some(stored) => {
                doc.set(r, Object::Stream(stored));
                stored_again += 1;
            }
            // A dictionary that names itself isn't something that uses one.
            None if dictionaries.contains(&r.num) => {}
            None => stuck += 1,
        }
    }
    let after = kept(doc);
    if dictionaries.iter().any(|n| after.contains(n)) {
        return Err(crate::RedactError::SharedJbig2Globals(stuck));
    }
    // What couldn't be stored has left with the dictionaries, a dictionary that names itself say:
    // it isn't counted.
    Ok(stored_again)
}
