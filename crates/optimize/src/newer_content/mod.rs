//! Content newer than a PDF version: every key, name value, flag and stream construct ISO 32000-2
//! marks as introduced in PDF 1.4 or later, found by walking every object with the context it is
//! reached in (the same key can be 1.4 in one dictionary and 2.0 in another).
//!
//! Report only: nothing is converted. Constructs before 1.4 are not told apart, the encryption
//! dictionary is left to the optimizer's own check, and ICC profile versions and file-structure
//! features (object streams, cross-reference streams) are not counted.

mod checks;
mod content;
mod ctx;
mod joined;
mod keys;
mod scan;

use std::collections::BTreeMap;
use std::fmt;

use pdfcraft_cos::{Document, MAX_PAGE_DECODED, ObjRef, Object};

use joined::Decoder;
use scan::{Found, Notes};

/// The most stream data one check decodes, over all its streams. Past it, streams are counted in
/// [`ContentVersions::unread`].
const MAX_DECODED: usize = MAX_PAGE_DECODED;

/// The most bytes of streams several pages share that are scanned again, joined into a later
/// page's content. Past it, the stream is left out of that page and counted unread.
const MAX_REJOINED: usize = MAX_PAGE_DECODED.saturating_mul(4);

/// The most bytes of one page's joined content held at once. Past it, a stream is left out of
/// that page and counted unread.
const MAX_JOINED: usize = MAX_PAGE_DECODED;

/// One construct a document uses and the PDF version that introduced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewerContent {
    /// `"1.4"` to `"2.0"`.
    pub version: &'static str,
    /// A key (`/UserUnit`), a key and value (`/Filter /JPXDecode`), or a short description
    /// (`16-bit image samples`).
    pub construct: String,
    /// Objects that use it (the trailer counts as one).
    pub objects: usize,
    /// The lowest-numbered object using it; `None` when only the trailer does.
    pub example: Option<ObjRef>,
}

impl fmt::Display for NewerContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PDF {}: {} ({} object(s)", self.version, self.construct, self.objects)?;
        if let Some(r) = self.example {
            write!(f, ", e.g. {r}")?;
        }
        f.write_str(")")
    }
}

/// What [`content_versions`] found.
pub(crate) struct ContentVersions {
    /// Highest version first.
    requirements: Vec<NewerContent>,
    /// Content streams and font programs that could not be read, each counted once, so a
    /// construct in them may be missing from `requirements`.
    pub(crate) unread: usize,
}

impl ContentVersions {
    /// The requirements newer than `version` (`"1.4"` to `"2.0"`; any other names none).
    pub(crate) fn newer_than(&self, version: &str) -> Vec<NewerContent> {
        let Some(chosen) = code(version) else { return Vec::new() };
        self.requirements.iter().filter(|r| code(r.version).is_some_and(|v| v > chosen)).cloned().collect()
    }
}

const VERSIONS: [(u8, &str); 5] = [(14, "1.4"), (15, "1.5"), (16, "1.6"), (17, "1.7"), (20, "2.0")];

fn name(code: u8) -> Option<&'static str> {
    VERSIONS.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

fn code(name: &str) -> Option<u8> {
    VERSIONS.iter().find(|(_, n)| *n == name).map(|(c, _)| *c)
}

/// The constructs of PDF 1.4 and later that `doc` uses, calling `keep_going` before each stream
/// it decodes; `None` once that returns `false`.
pub(crate) fn content_versions(doc: &Document, keep_going: &mut dyn FnMut() -> bool) -> Option<ContentVersions> {
    let all = scan::objects(doc);
    let reached = scan::reached(doc, &all);
    let arrays = joined::content_arrays(doc, &all, &reached);
    let mut tally = Tally::default();
    let mut found = Found::new(doc);
    let mut decoder = Decoder::new(MAX_DECODED, MAX_REJOINED, MAX_JOINED, &arrays);
    scan::scan_dict(doc.trailer(), ctx::TRAILER, 0, &mut found);
    tally.add(&mut found.notes, None);
    for &r in &all {
        let bits = reached.get(&r).copied().unwrap_or(0);
        match &*doc.get(r) {
            Object::Dict(d) => {
                scan::scan_dict(d, bits, 0, &mut found);
                if let Some(members) = arrays.get(&r) {
                    let notes = decoder.joined(doc, members, keep_going, &mut content::scan_content)?;
                    found.notes.extend(notes);
                }
            }
            Object::Stream(s) => {
                scan::scan_dict(&s.dict, bits | ctx::STREAM, 0, &mut found);
                let content = (ctx::own(&s.dict, true) | bits) & ctx::CONTENT != 0;
                // A page-array piece only is read in `decoder.joined`; any other content use, as a
                // form or a page's sole stream, is read here too, decoded again within the allowance.
                if content && bits & ctx::ENCRYPT == 0 {
                    if !keep_going() {
                        return None;
                    }
                    if let Some(data) = decoder.read(r, s) {
                        content::scan_content(&data, &mut found.notes);
                    }
                }
            }
            _ => {}
        }
        tally.add(&mut found.notes, Some(r));
    }
    let mut fonts = std::mem::take(&mut found.fonts);
    fonts.sort_unstable();
    fonts.dedup();
    for r in fonts {
        if !keep_going() {
            return None;
        }
        if let Object::Stream(s) = &*doc.get(r)
            && let Some(data) = decoder.read(r, s)
        {
            content::scan_font(&data, &mut found.notes);
        }
        tally.add(&mut found.notes, Some(r));
    }
    Some(ContentVersions { requirements: tally.into_requirements(), unread: decoder.unread() })
}

/// Per `(version, construct)`: the objects using it and the lowest of them.
#[derive(Default)]
struct Tally(BTreeMap<(u8, String), (usize, Option<ObjRef>)>);

impl Tally {
    fn add(&mut self, notes: &mut Notes, r: Option<ObjRef>) {
        for key in std::mem::take(notes) {
            let e = self.0.entry(key).or_insert((0, None));
            e.0 = e.0.saturating_add(1);
            e.1 = match (e.1, r) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
    }

    fn into_requirements(self) -> Vec<NewerContent> {
        let mut out: Vec<NewerContent> = self
            .0
            .into_iter()
            .filter_map(|((v, construct), (objects, example))| Some(NewerContent { version: name(v)?, construct, objects, example }))
            .collect();
        // Highest version first; the map already orders constructs within a version.
        out.sort_by_key(|r| std::cmp::Reverse(code(r.version)));
        out
    }
}
