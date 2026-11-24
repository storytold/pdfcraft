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
mod keys;
mod scan;

use std::collections::BTreeMap;
use std::fmt;

use pdfcraft_cos::{Document, MAX_PAGE_DECODED, ObjRef, Object, Stream};

use scan::{Fonts, Notes};

/// The most stream data one check decodes, over all its streams. Past it, streams are counted in
/// [`ContentVersions::unread`].
const MAX_DECODED: usize = MAX_PAGE_DECODED;

/// One construct a document uses and the PDF version that introduced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirement {
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

impl fmt::Display for Requirement {
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
    requirements: Vec<Requirement>,
    /// Content streams and font programs that could not be read, so a construct in them may be
    /// missing from `requirements`.
    pub(crate) unread: usize,
}

impl ContentVersions {
    /// The requirements newer than `version` (`"1.4"` to `"2.0"`; any other names none).
    pub(crate) fn newer_than(&self, version: &str) -> Vec<Requirement> {
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
    let mut tally = Tally::default();
    let mut notes = Notes::new();
    let mut fonts = Fonts::new();
    let mut decoder = Decoder { left: MAX_DECODED, unread: 0 };
    scan::scan_dict(doc.trailer(), ctx::TRAILER, 0, &mut notes, &mut fonts);
    tally.add(&mut notes, None);
    for &r in &all {
        let bits = reached.get(&r).copied().unwrap_or(0);
        let obj = doc.get(r);
        match &*obj {
            Object::Dict(d) => scan::scan_dict(d, bits, 0, &mut notes, &mut fonts),
            Object::Stream(s) => {
                scan::scan_dict(&s.dict, bits | ctx::STREAM, 0, &mut notes, &mut fonts);
                if (ctx::own(&s.dict, true) | bits) & ctx::CONTENT != 0 && bits & ctx::ENCRYPT == 0 {
                    if !keep_going() {
                        return None;
                    }
                    if let Some(data) = decoder.read(s) {
                        content::scan_content(&data, &mut notes);
                    }
                }
            }
            _ => {}
        }
        tally.add(&mut notes, Some(r));
    }
    fonts.sort_unstable();
    fonts.dedup();
    for r in fonts {
        if !keep_going() {
            return None;
        }
        if let Object::Stream(s) = &*doc.get(r)
            && let Some(data) = decoder.read(s)
        {
            content::scan_font(&data, &mut notes);
        }
        tally.add(&mut notes, Some(r));
    }
    Some(ContentVersions { requirements: tally.into_requirements(), unread: decoder.unread })
}

/// Decodes streams within the check's allowance, counting those it cannot read.
struct Decoder {
    left: usize,
    unread: usize,
}

impl Decoder {
    fn read(&mut self, s: &Stream) -> Option<Vec<u8>> {
        match s.decoded_within(self.left) {
            Ok(data) => {
                self.left = self.left.saturating_sub(data.len());
                Some(data)
            }
            Err(_) => {
                self.unread = self.unread.saturating_add(1);
                None
            }
        }
    }
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

    fn into_requirements(self) -> Vec<Requirement> {
        let mut out: Vec<Requirement> = self
            .0
            .into_iter()
            .filter_map(|((v, construct), (objects, example))| Some(Requirement { version: name(v)?, construct, objects, example }))
            .collect();
        // Highest version first; the map already orders constructs within a version.
        out.sort_by_key(|r| std::cmp::Reverse(code(r.version)));
        out
    }
}
