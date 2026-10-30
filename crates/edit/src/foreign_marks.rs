//! Watermarks other editors wrote (#4957): a page's content marked as an artifact of subtype
//! Watermark, `/Artifact <</Subtype /Watermark /Type /Pagination>> BDC … EMC` (ISO 32000-2
//! §14.8.2.2), which is how Acrobat and the other PDF editors mark theirs. The properties may be
//! inline or a name in the page's `/Properties`. Removing one cuts exactly those bytes out of the
//! content; everything else is copied as it was.
//!
//! Marked content does not save and restore the graphics state, so a watermark is cut out only
//! when nothing it does can reach what is drawn after it: every state change inside it sits
//! inside its own `q … Q`, and it opens and closes no text object or path the content around it
//! owns. One that does anything else is reported, not removed.

use std::fmt;
use std::ops::Range;

use super::place::{check, page_list};
use super::{Dict, Document, EditError, Object};
use pdfcraft_content::{Op, Pieces};

/// Marked-content scopes open at once beyond which a page is not read.
const MAX_SCOPES: usize = 1024;
/// The most decoded content bytes read from one document in one go, and operators in one page.
const MAX_CONTENT_BYTES: usize = 256 * 1024 * 1024;
const MAX_OPERATORS: usize = 5_000_000;
/// Pages looked at to tell whether a document has a watermark of this kind at all.
const MAX_DETECT_PAGES: usize = 5_000;
/// The deepest `q` nesting read (a page nested deeper than any real one is not read).
const MAX_DEPTH: usize = 100_000;

/// Why a page's watermark was not removed, or the page not checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// A content stream could not be read (damaged, too large).
    Unreadable,
    /// The content could not be read with certainty: bytes the parser skipped, an inline image
    /// whose end is a guess, marked-content scopes that do not pair up.
    Uncertain,
    /// A watermark is there, but cutting it out would change what is drawn after it.
    Unsafe,
}

impl fmt::Display for Blocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Blocked::Unreadable => "its content can't be read",
            Blocked::Uncertain => "its content can't be read with certainty",
            Blocked::Unsafe => "a watermark there changes what is drawn after it, so it can't be removed cleanly",
        })
    }
}

/// What [`remove_foreign_watermarks`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForeignWatermarks {
    /// Watermark artifacts removed.
    pub removed: usize,
    /// Pages (0-based) a watermark could not be removed from, or that could not be checked.
    pub blocked: Vec<(usize, Blocked)>,
}

/// A marked-content scope that is open.
struct Scope {
    watermark: bool,
    /// The graphics-state depth (`q` minus `Q`) where it opened, and the lowest it reached since.
    depth: usize,
    floor: usize,
    start: usize,
    /// Which operator opened it (counted from 1), to tell whether a state change came after.
    opened: usize,
    /// A text object or a path was open where it started.
    text: bool,
    path: bool,
}

/// Operators that change the graphics state, the colour state or the text state, or the clip: they
/// outlast the marked-content scope they are in, until the `Q` of the `q` around them.
fn changes_state(op: &[u8]) -> bool {
    matches!(
        op,
        b"cm"
            | b"w"
            | b"J"
            | b"j"
            | b"M"
            | b"d"
            | b"ri"
            | b"i"
            | b"gs"
            | b"CS"
            | b"cs"
            | b"SC"
            | b"SCN"
            | b"sc"
            | b"scn"
            | b"G"
            | b"g"
            | b"RG"
            | b"rg"
            | b"K"
            | b"k"
            | b"Tc"
            | b"Tw"
            | b"Tz"
            | b"TL"
            // `"` sets the word and character spacing and then shows text; `TD` sets the leading.
            | b"\""
            | b"TD"
            | b"Tf"
            | b"Tr"
            | b"Ts"
            | b"W"
            | b"W*"
    )
}

/// The value of `/Subtype` in `d`, following a reference, is the name Watermark.
fn subtype_is_watermark(doc: &Document, d: &Dict) -> bool {
    d.get(b"Subtype").map(|o| doc.resolve(o)).is_some_and(|v| v.as_name() == Some(b"Watermark"))
}

/// The page's `/Properties` resource dictionary, if it has one.
fn page_properties(doc: &Document, page: &pdfcraft_model::Page) -> Option<Dict> {
    page.dict
        .get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().and_then(|d| d.get(b"Properties").cloned()))
        .map(|p| doc.resolve(&p))
        .and_then(|p| p.as_dict().cloned())
}

/// Whether a `BDC` operator opens a watermark artifact.
fn opens_watermark(doc: &Document, properties: Option<&Dict>, op: &Op) -> bool {
    if op.name(0) != Some(b"Artifact") {
        return false;
    }
    match op.operands.get(1) {
        Some(Object::Dict(d)) => subtype_is_watermark(doc, d),
        Some(Object::Name(name)) => properties
            .and_then(|p| p.get(name))
            .map(|o| doc.resolve(o))
            .is_some_and(|resolved| resolved.as_dict().is_some_and(|d| subtype_is_watermark(doc, d))),
        _ => false,
    }
}

/// What a page's content holds.
enum Scan {
    /// No watermark artifact.
    Clean,
    /// Watermark artifacts: the byte ranges (in the joined content) that can be cut out, outermost
    /// ones only, and how many more cannot be.
    Found { spans: Vec<Range<usize>>, unsafe_left: usize },
    /// Not read with certainty.
    Uncertain,
}

fn scan_page(doc: &Document, page: &pdfcraft_model::Page, pieces: &Pieces) -> Scan {
    let properties = page_properties(doc, page);
    // Most pages have none: a watermark names itself in the content, or through a property whose
    // subtype says so. Neither: no need to read the content operator by operator. A name may be
    // spelled with `#` escapes (`/Water#6Dark` is `/Watermark`), so content with a `#` in it is
    // read in full: the absence of the plain spelling proves nothing there.
    let named = properties.as_ref().is_some_and(|p| p.iter().any(|(_, v)| doc.resolve(v).as_dict().is_some_and(|d| subtype_is_watermark(doc, d))));
    let data = pieces.data();
    if !named && !data.contains(&b'#') && !data.windows(9).any(|w| w == b"Watermark") {
        return Scan::Clean;
    }
    let parsed = pieces.parse();
    if parsed.skipped != 0 || parsed.ambiguous_images != 0 || parsed.ops.len() > MAX_OPERATORS {
        return Scan::Uncertain;
    }
    let (mut depth, mut stack, mut spans, mut unsafe_left) = (0usize, Vec::<Scope>::new(), Vec::<Range<usize>>::new(), 0usize);
    let (mut text, mut path) = (false, false);
    // Watermark scopes on `stack`, so telling whether one closing is the outermost is one compare,
    // however many other scopes are open.
    let mut open_watermarks = 0usize;
    // For each `q` depth, the count of the operator that last changed state there. A scope is
    // affected when it was opened before the last change at its own depth: one lookup, however
    // many scopes are open.
    let mut last_change: Vec<usize> = vec![0];
    for (index, op) in parsed.ops.iter().enumerate() {
        let count = index.saturating_add(1);
        let name = op.op.as_slice();
        match name {
            b"q" => {
                depth = depth.saturating_add(1);
                if depth > MAX_DEPTH {
                    return Scan::Uncertain;
                }
                if depth >= last_change.len() {
                    last_change.resize(depth + 1, 0);
                }
            }
            b"Q" => {
                depth = depth.saturating_sub(1);
                if let Some(top) = stack.last_mut() {
                    top.floor = top.floor.min(depth);
                }
            }
            b"BT" => text = true,
            b"ET" => text = false,
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" => path = true,
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => path = false,
            b"BMC" | b"BDC" => {
                if stack.len() >= MAX_SCOPES {
                    return Scan::Uncertain;
                }
                let watermark = name == b"BDC" && opens_watermark(doc, properties.as_ref(), op);
                if watermark {
                    open_watermarks = open_watermarks.saturating_add(1);
                }
                stack.push(Scope { watermark, depth, floor: depth, start: op.span.start, opened: count, text, path });
            }
            b"EMC" => {
                let Some(scope) = stack.pop() else { return Scan::Uncertain };
                if let Some(parent) = stack.last_mut() {
                    parent.floor = parent.floor.min(scope.floor);
                }
                if scope.watermark {
                    open_watermarks = open_watermarks.saturating_sub(1);
                }
                if scope.watermark && open_watermarks == 0 {
                    // Conservatively refuse a scope inside, opening or closing a text object or
                    // path: cutting it could change surrounding text placement or that path.
                    // `q … Q` restores text matrices inside a text object (errata 368), but not
                    // the current path; this guard does not rely on text-matrix restoration.
                    let effects = last_change.get(scope.depth).is_some_and(|last| *last > scope.opened);
                    let clean = depth == scope.depth && scope.floor >= scope.depth && !effects && !scope.text && !scope.path && !text && !path;
                    if clean {
                        spans.push(scope.start..op.span.end);
                    } else {
                        unsafe_left += 1;
                    }
                }
            }
            _ => {}
        }
        // A state change at a `q` depth reaches every scope open at that depth.
        if changes_state(name)
            && let Some(slot) = last_change.get_mut(depth)
        {
            *slot = count;
        }
    }
    if !stack.is_empty() {
        return Scan::Uncertain;
    }
    if spans.is_empty() && unsafe_left == 0 { Scan::Clean } else { Scan::Found { spans, unsafe_left } }
}

/// A page's content streams decoded, within what is left of `budget`.
fn page_streams(doc: &Document, page: &pdfcraft_model::Page, budget: &mut usize) -> Option<Vec<(Object, Vec<u8>)>> {
    let list: Vec<Object> = match page.dict.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut out = Vec::with_capacity(list.len().min(1024));
    for object in list {
        let resolved = doc.resolve(&object);
        let Object::Stream(stream) = &*resolved else { return None };
        let bytes = stream.decoded_within(*budget).ok()?;
        *budget = budget.checked_sub(bytes.len())?;
        out.push((object, bytes));
    }
    Some(out)
}

/// Whether the document may have a watermark artifact another editor wrote (for enabling Remove
/// watermark). `None` when the document is too large to look through, or a page can't be read:
/// then it may.
pub fn foreign_watermarks_present(doc: &Document) -> Option<bool> {
    let pages = page_list(doc);
    if pages.len() > MAX_DETECT_PAGES {
        return None;
    }
    let mut budget = MAX_CONTENT_BYTES;
    let mut unknown = false;
    for page in &pages {
        let Some(streams) = page_streams(doc, page, &mut budget) else {
            unknown = true;
            continue;
        };
        let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
        match scan_page(doc, page, &pieces) {
            Scan::Found { .. } => return Some(true),
            Scan::Uncertain => unknown = true,
            Scan::Clean => {}
        }
    }
    if unknown { None } else { Some(false) }
}

/// Remove the watermark artifacts other editors wrote from `pages` (0-based). A page whose
/// watermark can't be cut out cleanly, or whose content can't be read with certainty, is left as
/// it is and listed in the result, so the caller can say so: nothing is skipped silently. The
/// fonts, forms and images a removed watermark drew with stay in the page's resources, unused.
pub fn remove_foreign_watermarks(doc: &mut Document, pages: &[usize]) -> Result<ForeignWatermarks, EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    let mut result = ForeignWatermarks::default();
    let mut budget = MAX_CONTENT_BYTES;
    for &index in pages {
        let Some(page) = all.get(index) else { continue };
        let Some(streams) = page_streams(doc, page, &mut budget) else {
            result.blocked.push((index, Blocked::Unreadable));
            continue;
        };
        if streams.is_empty() {
            continue;
        }
        let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
        match scan_page(doc, page, &pieces) {
            Scan::Clean => {}
            Scan::Uncertain => result.blocked.push((index, Blocked::Uncertain)),
            Scan::Found { spans, unsafe_left } => {
                if unsafe_left > 0 {
                    result.blocked.push((index, Blocked::Unsafe));
                }
                if spans.is_empty() {
                    continue;
                }
                let data = pieces.splice(spans.iter().map(|r| (r.clone(), Vec::new())));
                let contents = crate::text::rewritten_contents(doc, &streams, data);
                doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), contents))?;
                result.removed += spans.len();
            }
        }
    }
    Ok(result)
}
