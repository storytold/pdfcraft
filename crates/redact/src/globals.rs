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

use pdfcraft_cos::{CosError, DECODE_PARMS_KEYS, Dict, Document, ImageBudget, MAX_JBIG2_GLOBALS, Object, Stream, reachable_objects};

/// The references followed from `/JBIG2Globals` towards a stream, so that a ring of them ends.
const MAX_HOPS: usize = 32;

/// The objects a full save would keep, by number: a reference reads the object of its number,
/// whatever generation it gives. Fails only when a full save couldn't number them all.
pub(crate) fn kept(doc: &Document) -> Result<HashSet<u32>, CosError> {
    reachable_objects(doc)
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

/// The most one run may spend finding what shares a dictionary with what left: objects fetched,
/// names remembered, and decoded data, a unit and one for each whole 4 KiB. Past it the run is refused
/// ([`RedactError::SharedJbig2Budget`](crate::RedactError::SharedJbig2Budget)). A scan reads a few
/// objects a page, and its pages stored again a unit for each 4 KiB of their samples and of the
/// dictionary they name, so a book of thousands of pages sharing a dictionary of a MiB or two stays
/// within it. The work of decoding, every filter stage's and the JBIG2 decoder's, is paid besides
/// from the run's image allowance (`ImageBudget`), which a unit of decoded data says nothing of.
pub(crate) const RUN_BUDGET: usize = 1 << 22;

/// What a parameters value names as `/JBIG2Globals`, and the key it is remembered by: a value the
/// file refers to, by the number of the object the references end at and whether it is read as a
/// list of entries.
pub(crate) type Listed = (Rc<[u32]>, Option<Key>);

/// A value the file refers to: the number of an object, and whether it is read as a list of entries.
pub(crate) type Key = (u32, bool);

/// How a value is read, and so what is kept of it: what a reference leads to is remembered for each.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum How {
    /// A size: a number, or null.
    Number,
    /// One filter: a name, or null.
    Name,
    /// `/Filter`: a name, or an array of names, or null for a chain longer than any decoder takes.
    Filter,
    /// `/DecodeParms` for so many filters.
    Parms(usize),
    /// One parameters entry: the numbers and flags decoders read, and `/JBIG2Globals` by reference.
    Entry,
    /// A number or a flag.
    Scalar,
    /// `/F`: whether it names anything (true) or is null.
    Present,
    /// `/ColorSpace`: whether it is known to have more than one component.
    Components,
    /// An ICC profile: its `/N`, or null.
    Profile,
}

/// What one redaction or Remove Hidden Information run reads, to find what shares a dictionary with
/// what left, against [`RUN_BUDGET`]. Each object is fetched at most once a run for each way it is
/// read, and kept only as what is read of it: a value many streams share, by reference, through a
/// chain of them or through an entry, costs it once however many there are. The exceptions are a
/// `/JBIG2Globals` chain named part way, which is followed again from there, and a ring of
/// references or a chain past the limit, read again (up to 33 fetches) wherever it is met; every
/// fetch counts.
pub(crate) struct Run {
    budget: usize,
    /// What was spent of the budget.
    pub(crate) spent: usize,
    /// What a reference leads to, as it is read, under every reference on the way.
    reduced: HashMap<(u32, How), Rc<Object>>,
    /// What a reference leads to names, under every reference on the way, with the key of the one
    /// that names what it ends at: aliases and chains find it, and are counted as that one.
    names: HashMap<Key, (Rc<[u32]>, Key)>,
    /// What a `/JBIG2Globals` reference leads to: it and each reference after it, by its number.
    chains: HashMap<u32, Rc<[u32]>>,
    /// What each dictionary was found to decode to, or its limit when it couldn't be: an image
    /// naming it may undo its filters again, which costs as much.
    sizes: HashMap<u32, usize>,
    /// How many values were read for their names rather than remembered.
    pub(crate) read: usize,
    /// How many objects were fetched.
    pub(crate) fetched: usize,
}

impl Run {
    pub(crate) fn new(budget: usize) -> Self {
        Run { budget, spent: 0, reduced: HashMap::new(), names: HashMap::new(), chains: HashMap::new(), sizes: HashMap::new(), read: 0, fetched: 0 }
    }

    /// Counts `units` against the budget.
    fn spend(&mut self, units: usize) -> Result<(), crate::RedactError> {
        self.spent = self.spent.saturating_add(units);
        if self.spent > self.budget { Err(crate::RedactError::SharedJbig2Budget) } else { Ok(()) }
    }

    /// Fetches the object `r` names, counted.
    fn fetch(&mut self, doc: &Document, r: pdfcraft_cos::ObjRef) -> Result<Arc<Object>, crate::RedactError> {
        self.spend(1)?;
        self.fetched += 1;
        Ok(doc.get(r))
    }

    /// What `v` leads to through references, as `followed` goes, kept as `read` makes it. Each
    /// reference is looked up before it is fetched, and what it leads to is remembered under every
    /// one on the way, so one remembered costs nothing more, and an alias or a chain only the
    /// references before it. A ring, or a chain past the limit, is read as the reference it ends at.
    fn follow(
        &mut self,
        doc: &Document,
        v: &Object,
        how: How,
        read: impl FnOnce(&mut Self, &Document, &Object) -> Result<Object, crate::RedactError>,
    ) -> Result<Rc<Object>, crate::RedactError> {
        let mut keys: Vec<(u32, How)> = Vec::new();
        let mut fetched: Option<Arc<Object>> = None;
        for _ in 0..=MAX_HOPS {
            let Some(r) = fetched.as_deref().unwrap_or(v).as_ref() else { break };
            if let Some(hit) = self.reduced.get(&(r.num, how)).cloned() {
                for k in keys {
                    self.reduced.insert(k, hit.clone());
                }
                return Ok(hit);
            }
            keys.push((r.num, how));
            fetched = Some(self.fetch(doc, r)?);
        }
        let made = Rc::new(read(self, doc, fetched.as_deref().unwrap_or(v))?);
        for k in keys {
            self.reduced.insert(k, made.clone());
        }
        Ok(made)
    }

    /// What `s` names as `/JBIG2Globals`, and each reference on the way to one: in any entry of its
    /// parameters, whichever filter that entry belongs to and however the filters are given, one
    /// list for each key that gives it parameters. A reader may use less of this, but leaving a
    /// dictionary behind is the failure that matters here.
    pub(crate) fn names_of(&mut self, doc: &Document, s: &Stream) -> Result<Vec<Listed>, crate::RedactError> {
        // `/FDecodeParms` too: those of data in another file, which a reader may draw instead.
        [b"DecodeParms".as_slice(), b"DP", b"FDecodeParms"]
            .into_iter()
            .filter_map(|key| s.dict.get(key))
            .map(|v| self.names_in(doc, v, true))
            .collect()
    }

    /// What `v` names: one entry, or (`list`) an array of them or one.
    fn names_in(&mut self, doc: &Document, v: &Object, list: bool) -> Result<Listed, crate::RedactError> {
        let mut keys: Vec<Key> = Vec::new();
        let mut fetched: Option<Arc<Object>> = None;
        for _ in 0..=MAX_HOPS {
            let Some(r) = fetched.as_deref().unwrap_or(v).as_ref() else { break };
            let key = (r.num, list);
            if let Some((hit, at)) = self.names.get(&key).cloned() {
                for k in keys {
                    self.names.insert(k, (hit.clone(), at));
                }
                return Ok((hit, Some(at)));
            }
            keys.push(key);
            fetched = Some(self.fetch(doc, r)?);
        }
        self.read += 1;
        let held = fetched.as_deref().unwrap_or(v);
        let names: Rc<[u32]> = match (held, list) {
            (Object::Array(entries), true) => {
                // A set as it is read, so many entries naming the same holds them once.
                let mut all = std::collections::BTreeSet::new();
                for e in entries {
                    all.extend(self.names_in(doc, e, false)?.0.iter());
                }
                all.into_iter().collect()
            }
            (entry, _) => self.entry_names(doc, entry)?.into(),
        };
        // Remembered when the references end at an object, under the last of them, and counted
        // against the budget as held; a ring, or a chain past the limit, is read again where met.
        let at = keys.last().copied().filter(|_| !matches!(held, Object::Ref(_)));
        if let Some(at) = at {
            self.spend(names.len())?;
            for k in keys {
                self.names.insert(k, (names.clone(), at));
            }
        }
        Ok((names, at))
    }

    /// What one parameters entry names as `/JBIG2Globals`, and each reference on the way to it.
    fn entry_names(&mut self, doc: &Document, entry: &Object) -> Result<Vec<u32>, crate::RedactError> {
        match entry.as_dict().and_then(|d| d.get(b"JBIG2Globals")).and_then(Object::as_ref) {
            Some(start) => Ok(self.chain(doc, start)?.to_vec()),
            None => Ok(Vec::new()),
        }
    }

    /// `start` and each reference after it, up to the limit, remembered under `start` for the run,
    /// so entries naming it, directly or not, fetch it once however many there are; a chain that
    /// reaches the start of one already remembered takes what is known from there.
    fn chain(&mut self, doc: &Document, start: pdfcraft_cos::ObjRef) -> Result<Rc<[u32]>, crate::RedactError> {
        if let Some(hit) = self.chains.get(&start.num) {
            return Ok(hit.clone());
        }
        let mut found = Vec::new();
        let mut at = start;
        for _ in 0..=MAX_HOPS {
            if let Some(rest) = self.chains.get(&at.num) {
                found.extend(rest.iter());
                break;
            }
            found.push(at.num);
            match &*self.fetch(doc, at)? {
                Object::Ref(next) => at = *next,
                _ => break,
            }
        }
        found.truncate(MAX_HOPS + 1);
        let found: Rc<[u32]> = found.into();
        self.spend(found.len())?;
        self.chains.insert(start.num, found.clone());
        Ok(found)
    }

    /// A size through any references: a number, or null.
    fn number(&mut self, doc: &Document, v: &Object) -> Result<Object, crate::RedactError> {
        let read = |_: &mut Self, _: &Document, o: &Object| Ok(if matches!(o, Object::Int(_) | Object::Real(_)) { o.clone() } else { Object::Null });
        Ok((*self.follow(doc, v, How::Number, read)?).clone())
    }

    /// A `/Filter` through any references, as a decoder reads it: a name a decoder knows, or an array
    /// of them, each entry through references too. Anything else is null, a chain longer than any
    /// decoder takes (`MAX_CHAIN`, and a `/Crypt`) included, so a stream that names it is read as one
    /// with no filter.
    fn filter(&mut self, doc: &Document, v: &Object) -> Result<Object, crate::RedactError> {
        // A name a decoder knows; any other is no filter, and isn't copied, however long.
        fn name(_: &mut Run, _: &Document, o: &Object) -> Result<Object, crate::RedactError> {
            let known = |n: &[u8]| n.len() <= 16 && !matches!(pdfcraft_filters::Filter::from_name(n), pdfcraft_filters::Filter::Unknown(_));
            Ok(match o {
                Object::Name(n) if known(n) => o.clone(),
                _ => Object::Null,
            })
        }
        let read = |run: &mut Self, doc: &Document, o: &Object| match o {
            Object::Array(a) if a.len() > pdfcraft_filters::MAX_CHAIN + 1 => Ok(Object::Null),
            Object::Array(a) => {
                let mut names = Vec::with_capacity(a.len());
                for e in a {
                    names.push((*run.follow(doc, e, How::Name, name)?).clone());
                }
                Ok(Object::Array(names))
            }
            other => name(run, doc, other),
        };
        Ok((*self.follow(doc, v, How::Filter, read)?).clone())
    }

    /// `/DecodeParms` as a reader takes them, through any references, for `filters` filters: of each
    /// entry, the numbers and flags the decoders read ([`DECODE_PARMS_KEYS`]), and `/JBIG2Globals` when
    /// it is a reference (a direct value is no stream, so no decoder reads it, and it may be large).
    /// An entry that isn't a dictionary is null, as `Document::with_direct_parms` reads them.
    fn parms(&mut self, doc: &Document, v: &Object, filters: usize) -> Result<Object, crate::RedactError> {
        fn scalar(_: &mut Run, _: &Document, o: &Object) -> Result<Object, crate::RedactError> {
            Ok(if matches!(o, Object::Int(_) | Object::Real(_) | Object::Bool(_)) { o.clone() } else { Object::Null })
        }
        fn entry(run: &mut Run, doc: &Document, o: &Object) -> Result<Object, crate::RedactError> {
            let Object::Dict(d) = o else { return Ok(Object::Null) };
            let mut made = Dict::new();
            for key in DECODE_PARMS_KEYS {
                let Some(value) = d.get(key) else { continue };
                let value = run.follow(doc, value, How::Scalar, scalar)?;
                if !matches!(*value, Object::Null) {
                    made.set(key.to_vec(), (*value).clone());
                }
            }
            if let Some(globals @ Object::Ref(_)) = d.get(b"JBIG2Globals") {
                made.set(b"JBIG2Globals".to_vec(), globals.clone());
            }
            Ok(Object::Dict(made))
        }
        let read = move |run: &mut Self, doc: &Document, o: &Object| match o {
            Object::Array(a) => {
                let mut entries = Vec::new();
                for e in a.iter().take(filters) {
                    entries.push((*run.follow(doc, e, How::Entry, entry)?).clone());
                }
                Ok(Object::Array(entries))
            }
            other => entry(run, doc, other),
        };
        Ok((*self.follow(doc, v, How::Parms(filters), read)?).clone())
    }

    /// Whether `s`'s data is in another file, which a reader may draw instead of what is here: a `/F`
    /// that isn't null, through any references (a null entry is as if absent, and a reference to no
    /// object is null, ISO 32000-2 §7.3.7 and §7.3.10). A ring of references counts as a file.
    fn elsewhere(&mut self, doc: &Document, s: &Stream) -> Result<bool, crate::RedactError> {
        let Some(f) = s.dict.get(b"F") else { return Ok(false) };
        let read = |_: &mut Self, _: &Document, o: &Object| Ok(Object::Bool(!matches!(o, Object::Null)));
        Ok(matches!(*self.follow(doc, f, How::Present, read)?, Object::Bool(true)))
    }

    /// Whether `s`'s colour space is known to have more than one component, through any references:
    /// a JBIG2 image is drawn as one whatever its dictionary says, so such an image read as one bit
    /// would show otherwise. An unknown colour space isn't known to have more.
    fn several_components(&mut self, doc: &Document, s: &Stream) -> Result<bool, crate::RedactError> {
        let Some(cs) = s.dict.get(b"ColorSpace") else { return Ok(false) };
        fn profile(_: &mut Run, _: &Document, o: &Object) -> Result<Object, crate::RedactError> {
            // /N lives in the ICC profile stream, and shall be 1, 3 or 4 (ISO 32000-2 §8.6.5.5).
            let n = match o {
                Object::Stream(icc) => icc.dict.int(b"N").filter(|n| matches!(n, 1 | 3 | 4)),
                _ => None,
            };
            Ok(n.map_or(Object::Null, Object::Int))
        }
        let read = |run: &mut Self, doc: &Document, o: &Object| {
            let name = match o {
                Object::Name(n) => n.as_slice(),
                Object::Array(a) => match a.first().and_then(Object::as_name) {
                    Some(n) => n,
                    None => return Ok(Object::Bool(false)),
                },
                _ => return Ok(Object::Bool(false)),
            };
            Ok(Object::Bool(match name {
                b"DeviceRGB" | b"RGB" | b"CalRGB" | b"Lab" | b"DeviceCMYK" | b"CMYK" => true,
                b"ICCBased" => match o.as_array().and_then(|a| a.get(1)) {
                    Some(entry) => run.follow(doc, entry, How::Profile, profile)?.as_int().is_some_and(|n| n > 1),
                    None => false,
                },
                _ => false,
            }))
        };
        Ok(matches!(*self.follow(doc, cs, How::Components, read)?, Object::Bool(true)))
    }

    /// `s` with what `decoded_image` and `decoded_within` read as given made direct: its size,
    /// filters and their parameters, each kept small, however often the file refers to a large object.
    pub(crate) fn readable(&mut self, doc: &Document, s: &Stream) -> Result<Stream, crate::RedactError> {
        let mut readable = s.clone();
        for key in [b"Width".as_slice(), b"Height"] {
            if let Some(v) = s.dict.get(key) {
                let number = self.number(doc, v)?;
                readable.dict.set(key.to_vec(), number);
            }
        }
        if let Some(v) = s.dict.get(b"Filter") {
            let filter = self.filter(doc, v)?;
            readable.dict.set(b"Filter".to_vec(), filter);
        }
        if let Some(v) = s.dict.get(b"DecodeParms") {
            let filters = match readable.dict.get(b"Filter") {
                Some(Object::Array(a)) => a.len(),
                Some(Object::Name(_)) => 1,
                _ => 0,
            };
            let parms = self.parms(doc, v, filters)?;
            readable.dict.set(b"DecodeParms".to_vec(), parms);
        }
        Ok(readable)
    }

    /// Counts what was decoded: a unit, and one more for each whole 4 KiB.
    pub(crate) fn decoded(&mut self, bytes: usize) -> Result<(), crate::RedactError> {
        self.spend(bytes / 4096 + 1)
    }
}

/// What dictionary `num` holds, to know a copy of it by: a file put together from pages split apart
/// has one per page, each byte for byte the others.
///
/// Decoded within `images`, the run's image allowance, which pays for what every filter stage reads
/// and makes: what the last one makes says nothing of the work (a predictor over what three Flates
/// inflate, which hex then skips as white space, ends at nothing). Past it the run is refused
/// ([`RedactError::ImagesPastAllowance`](crate::RedactError::ImagesPastAllowance)).
pub(crate) fn content(run: &mut Run, doc: &Document, num: u32, images: &mut ImageBudget) -> Result<Option<u64>, crate::RedactError> {
    let Ok(object) = doc.try_get(num) else { return Ok(None) };
    let Object::Stream(s) = &*object else { return Ok(None) };
    let readable = run.readable(doc, s)?;
    let refused = images.refused();
    let data = images.decode_filters(doc, &readable, MAX_JBIG2_GLOBALS);
    if data.is_err() && images.refused() > refused {
        return Err(crate::RedactError::ImagesPastAllowance);
    }
    let mut hash = DefaultHasher::new();
    match &data {
        Ok(data) => {
            run.decoded(data.len())?;
            run.sizes.insert(num, data.len());
            data.hash(&mut hash);
        }
        // What it took to find out it can't be read: as much as it was allowed to decode.
        Err(_) => {
            run.decoded(MAX_JBIG2_GLOBALS)?;
            run.sizes.insert(num, MAX_JBIG2_GLOBALS);
            s.raw[..].hash(&mut hash);
        }
    }
    Ok(Some(hash.finish()))
}

/// JBIG2 stream `s` stored alone, as a cleared copy is: what it shows unchanged and no dictionary
/// referred to. `None` for a stream that isn't JBIG2, whose decoder took no dictionary and whose
/// samples another filter may give otherwise; for one past the size of an image redaction clears,
/// or known to have more than one component, which JBIG2 is drawn as one of whatever the dictionary
/// says; for one whose data is in another file (`/F`), which readers draw instead of what is here;
/// and for one that can't be decoded.
pub(crate) fn without_dictionary(run: &mut Run, doc: &Document, s: &Stream, images: &mut ImageBudget) -> Result<Option<Stream>, crate::RedactError> {
    if run.elsewhere(doc, s)? {
        return Ok(None);
    }
    let readable = run.readable(doc, s)?;
    let jbig2 = match readable.dict.get(b"Filter") {
        Some(Object::Array(a)) => a.iter().any(|f| f.as_name() == Some(b"JBIG2Decode".as_slice())),
        Some(f) => f.as_name() == Some(b"JBIG2Decode".as_slice()),
        None => false,
    };
    if !jbig2 || run.several_components(doc, s)? {
        return Ok(None);
    }
    let side = |key: &[u8]| readable.dict.int(key).and_then(|v| usize::try_from(v).ok());
    let (Some(w), Some(h)) = (side(b"Width"), side(b"Height")) else { return Ok(None) };
    if (w as u64).saturating_mul(h as u64) > crate::image::MAX_PIXELS {
        return Ok(None);
    }
    // As for `clear`: the filters before the JBIG2 decoder may give up to three bytes for each one
    // of the raster and a byte a row.
    let Some(allowance) =
        w.div_ceil(8).checked_mul(h).and_then(|n| n.checked_add(h)).and_then(|n| n.checked_mul(3)).and_then(|n| n.checked_add(4096))
    else {
        return Ok(None);
    };
    // Decoding it undoes the filters of each dictionary it names again when cos doesn't hold their
    // bytes: charged as much as each was found to decode to.
    let mut named = std::collections::BTreeSet::new();
    for (list, _) in run.names_of(doc, s)? {
        named.extend(list.iter().copied());
    }
    let undone = named.iter().filter_map(|n| run.sizes.get(n)).fold(0usize, |sum, size| sum.saturating_add(*size));
    if undone > 0 {
        run.decoded(undone)?;
    }
    // Through the run's image allowance, which the decoder's own work, the dictionary's filters
    // when they are undone again and the pixels are paid from. Past it the run is refused, as one
    // clearing too many images is.
    let refused = images.refused();
    let data = match images.decode(doc, &readable, allowance) {
        Ok(data) => data,
        Err(_) if images.refused() > refused => return Err(crate::RedactError::ImagesPastAllowance),
        Err(_) => {
            // What it took to find out it can't be read: as much as it was allowed to decode.
            run.decoded(allowance)?;
            return Ok(None);
        }
    };
    run.decoded(data.len())?;
    let mut dict = readable.dict.clone();
    // `/FFilter` and `/FDecodeParms` too: with no data in another file they mean nothing, but
    // `names_of` reads them.
    for key in [b"Length".as_slice(), b"DecodeParms", b"DP", b"FFilter", b"FDecodeParms"] {
        dict.remove(key);
    }
    dict.set(b"BitsPerComponent".to_vec(), Object::Int(1));
    Ok(Some(crate::image::smallest(dict, &data, w, h, true, true)))
}

/// Whether dictionary `n` is one that left (`left`), or a copy of one (`contents`); what each
/// object holds is found once a run (`read`).
fn matches_left(
    run: &mut Run,
    doc: &Document,
    n: u32,
    left: &HashSet<u32>,
    contents: &HashSet<u64>,
    read: &mut HashMap<u32, Option<u64>>,
    images: &mut ImageBudget,
) -> Result<bool, crate::RedactError> {
    if left.contains(&n) {
        return Ok(true);
    }
    let held = match read.get(&n) {
        Some(held) => *held,
        None => {
            let held = content(run, doc, n, images)?;
            read.insert(n, held);
            held
        }
    };
    Ok(held.is_some_and(|c| contents.contains(&c)))
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
/// What it reads and decodes is counted against [`RUN_BUDGET`] for the whole run, and each
/// dictionary and image is decoded within `images`, the run's image allowance.
///
/// Errors: [`RedactError::SharedJbig2Globals`](crate::RedactError::SharedJbig2Globals) when a dictionary would
/// still be kept, as what names it can't be stored without it (counted) or something else refers to
/// it (0 when that is all); [`RedactError::SharedJbig2Budget`](crate::RedactError::SharedJbig2Budget) when
/// finding that would spend more than the budget; [`RedactError::ImagesPastAllowance`](crate::RedactError::ImagesPastAllowance)
/// when decoding the dictionaries and images would spend more than `images` has left. Such an image may still be drawn, on a page no mark
/// reached, so it isn't removed, and `doc` is part changed by then: a caller works on a copy it can drop.
pub(crate) fn unshare(doc: &mut Document, before: &HashSet<u32>, images: &mut ImageBudget) -> Result<usize, crate::RedactError> {
    let now = kept(doc)?;
    let mut run = Run::new(RUN_BUDGET);
    let mut dictionaries: HashSet<u32> = HashSet::new();
    // A list the file refers to is added once, however many streams share it.
    let mut added: HashSet<Key> = HashSet::new();
    for &num in before.difference(&now) {
        if let Ok(object) = doc.try_get(num)
            && let Object::Stream(s) = &*object
        {
            for (list, key) in run.names_of(doc, s)? {
                if key.is_none_or(|k| added.insert(k)) {
                    dictionaries.extend(list.iter());
                }
            }
        }
    }
    if dictionaries.is_empty() {
        return Ok(0);
    }
    let mut contents: HashSet<u64> = HashSet::new();
    for &n in &dictionaries {
        if let Some(c) = content(&mut run, doc, n, images)? {
            contents.insert(c);
        }
    }
    let mut read: HashMap<u32, Option<u64>> = HashMap::new();
    // Whether a name matches doesn't depend on order: it is a dictionary that left, or a copy of one
    // (what is added below is only ever such a copy). So what a shared list matches is found once.
    let left = dictionaries.clone();
    let mut matches: HashMap<Key, Rc<[u32]>> = HashMap::new();
    added.clear();
    let mut users = Vec::new();
    for (r, object) in doc.scan_objects() {
        let Object::Stream(s) = &*object else { continue };
        if !now.contains(&r.num) {
            continue;
        }
        // Only what matched has to leave, a copy found here included. Another dictionary this also
        // names was shared with nothing that left: what else uses it may keep it, whichever comes
        // first in the file.
        let mut uses = false;
        for (list, key) in run.names_of(doc, s)? {
            let matched = match key.and_then(|k| matches.get(&k)) {
                Some(hit) => hit.clone(),
                None => {
                    let mut found = Vec::new();
                    for &n in list.iter() {
                        if matches_left(&mut run, doc, n, &left, &contents, &mut read, images)? {
                            found.push(n);
                        }
                    }
                    let found: Rc<[u32]> = found.into();
                    if let Some(k) = key {
                        run.spend(found.len())?;
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
        match without_dictionary(&mut run, doc, s, images)? {
            Some(stored) => {
                doc.set(r, Object::Stream(stored));
                stored_again += 1;
            }
            // A dictionary that names itself isn't something that uses one.
            None if dictionaries.contains(&r.num) => {}
            None => stuck += 1,
        }
    }
    let after = kept(doc)?;
    if dictionaries.iter().any(|n| after.contains(n)) {
        return Err(crate::RedactError::SharedJbig2Globals(stuck));
    }
    // What couldn't be stored has left with the dictionaries, a dictionary that names itself say:
    // it isn't counted.
    Ok(stored_again)
}
