//! Watermarks other editors wrote (#4957): a page's content marked as an artifact of subtype
//! Watermark, `/Artifact <</Subtype /Watermark /Type /Pagination>> BDC … EMC` (ISO 32000-2
//! §14.8.2.2), which is how Acrobat and the other PDF editors mark theirs. The properties may be
//! inline or a name in the page's `/Properties`. Removing one cuts exactly those bytes out of the
//! content; everything else is copied as it was.

use std::ops::Range;

use super::*;
use pdfcraft_content::Pieces;

/// Marked-content scopes open at once beyond which a page is not read.
const MAX_SCOPES: usize = 1024;

/// A marked-content scope that is open.
struct Scope {
    watermark: bool,
    /// The graphics-state depth (`q` minus `Q`) where it opened, and the lowest it reached since.
    depth: usize,
    floor: usize,
    start: usize,
}

/// Whether a `BDC` operator opens a watermark artifact.
fn opens_watermark(doc: &Document, properties: Option<&Dict>, op: &pdfcraft_content::Op) -> bool {
    if op.name(0) != Some(b"Artifact") {
        return false;
    }
    let is_watermark = |d: &Dict| d.name(b"Subtype") == Some(b"Watermark");
    match op.operands.get(1) {
        Some(Object::Dict(d)) => is_watermark(d),
        Some(Object::Name(name)) => {
            properties.and_then(|p| p.get(name)).map(|o| doc.resolve(o)).is_some_and(|resolved| resolved.as_dict().is_some_and(is_watermark))
        }
        _ => false,
    }
}

/// The byte ranges (in the joined content) of the watermark artifacts on a page, outermost ones
/// only. `None` when the content can't be read with certainty (bytes the parser skipped, an inline
/// image whose end is a guess, marked-content scopes that don't pair up). A watermark whose
/// `q`/`Q` don't balance inside it is left, since cutting it out would change the graphics state
/// of everything after it.
fn watermark_spans(doc: &Document, page: &pdfcraft_model::Page, pieces: &Pieces) -> Option<Vec<Range<usize>>> {
    let parsed = pieces.parse();
    if parsed.skipped != 0 || parsed.ambiguous_images != 0 {
        return None;
    }
    let properties = page
        .dict
        .get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().and_then(|d| d.get(b"Properties").cloned()))
        .map(|p| doc.resolve(&p))
        .and_then(|p| p.as_dict().cloned());
    let (mut depth, mut stack, mut spans): (usize, Vec<Scope>, Vec<Range<usize>>) = (0, Vec::new(), Vec::new());
    for op in &parsed.ops {
        match op.op.as_slice() {
            b"q" => depth = depth.saturating_add(1),
            b"Q" => {
                depth = depth.saturating_sub(1);
                if let Some(top) = stack.last_mut() {
                    top.floor = top.floor.min(depth);
                }
            }
            b"BMC" | b"BDC" => {
                if stack.len() >= MAX_SCOPES {
                    return None;
                }
                let watermark = op.is("BDC") && opens_watermark(doc, properties.as_ref(), op);
                stack.push(Scope { watermark, depth, floor: depth, start: op.span.start });
            }
            b"EMC" => {
                let scope = stack.pop()?;
                if let Some(parent) = stack.last_mut() {
                    parent.floor = parent.floor.min(scope.floor);
                }
                // Inside a watermark that is cut out whole, nothing is cut separately.
                if scope.watermark && depth == scope.depth && scope.floor >= scope.depth && !stack.iter().any(|s| s.watermark) {
                    spans.push(scope.start..op.span.end);
                }
            }
            _ => {}
        }
    }
    stack.is_empty().then_some(spans)
}

/// Remove the watermark artifacts other editors wrote from `pages` (0-based); returns how many
/// were removed. A page whose content can't be read with certainty is left as it is. The fonts,
/// forms and images the watermark drew with stay in the page's resources, unused.
pub fn remove_foreign_watermarks(doc: &mut Document, pages: &[usize]) -> Result<usize, EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    let mut removed = 0;
    for &index in pages {
        let Some(page) = all.get(index) else { continue };
        // A page too large or damaged to read here has nothing removed, and does not stop the others.
        let Ok(streams) = crate::objects::checked_streams(doc, index) else { continue };
        if streams.is_empty() {
            continue;
        }
        let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
        let Some(spans) = watermark_spans(doc, page, &pieces).filter(|s| !s.is_empty()) else { continue };
        let data = pieces.splice(spans.iter().map(|r| (r.clone(), Vec::new())));
        let contents = crate::text::rewritten_contents(doc, &streams, data);
        doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), contents))?;
        removed += spans.len();
    }
    Ok(removed)
}
