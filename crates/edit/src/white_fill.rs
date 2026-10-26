//! Place a normal background above a leading white page fill, without recolouring foreground.
//! Only a small, balanced device-colour prefix is accepted. Unknown content is left alone.

use super::*;
use pdfcraft_content::{Matrix, Pieces, contains};

struct Boundary {
    index: usize,
    offset: usize,
    bytes: Vec<u8>,
}

/// A conservative subset of page backgrounds: no clipping, graphics states, marked content,
/// strokes, patterns or other painting; the graphics stack and CTM must be restored before
/// insertion. Device colour overrides could turn nominal white into another colour.
fn boundary(doc: &Document, page: &pdfcraft_model::Page, list: &[Object]) -> Option<Boundary> {
    let resources = page.dict.get(b"Resources").map(|r| doc.resolve(r));
    let spaces = resources.as_deref().and_then(Object::as_dict).and_then(|d| d.get(b"ColorSpace")).map(|r| doc.resolve(r));
    if spaces
        .as_deref()
        .and_then(Object::as_dict)
        .is_some_and(|d| [b"DefaultGray".as_slice(), b"DefaultRGB", b"DefaultCMYK"].iter().any(|k| d.get(k).is_some()))
    {
        return None;
    }
    let mut data = Vec::new();
    let mut indices = Vec::new();
    let mut read = 0usize;
    for (i, object) in list.iter().enumerate() {
        if stream_tag(doc, object).is_some() {
            continue;
        }
        let resolved = doc.resolve(object);
        let Object::Stream(stream) = &*resolved else { return None };
        let Ok(bytes) = stream.decoded_strict_within(COVER_SCAN_BYTES.saturating_sub(read)) else { break };
        read = read.checked_add(bytes.len().checked_add(1)?)?;
        if read > COVER_SCAN_BYTES {
            break;
        }
        indices.push(i);
        data.push(bytes);
    }
    let pieces = Pieces::join(&data);
    let parsed = pieces.parse();
    if parsed.skipped != 0 {
        return None;
    }
    let mut ctm = Matrix::IDENTITY;
    let mut white = false;
    let mut stack = Vec::new();
    let mut rect = None;
    let mut painted = false;
    for op in parsed.ops {
        if painted && op.op != b"Q" {
            return None;
        }
        match op.op.as_slice() {
            b"q" if op.operands.is_empty() && stack.len() < 64 => stack.push((ctm, white)),
            b"Q" if op.operands.is_empty() => (ctm, white) = stack.pop()?,
            b"cm" if op.operands.len() == 6 => {
                ctm = Matrix::from_operands(&op.operands)?.then(&ctm);
                if !ctm.0.iter().all(|v| v.is_finite()) {
                    return None;
                }
            }
            b"g" if op.operands.len() == 1 => white = op.nums::<1>()? == [1.0],
            b"rg" if op.operands.len() == 3 => white = op.nums::<3>()? == [1.0; 3],
            b"k" if op.operands.len() == 4 => white = op.nums::<4>()? == [0.0; 4],
            b"re" if op.operands.len() == 4 && rect.is_none() => {
                let [x, y, w, h] = op.nums::<4>()?;
                if ![x, y, w, h, x + w, y + h].iter().all(|v| v.is_finite()) {
                    return None;
                }
                let [a, b, c, d, _, _] = ctm.0;
                // No approximate turns: a bounding box alone cannot prove coverage.
                if !(b == 0.0 && c == 0.0 || a == 0.0 && d == 0.0) {
                    return None;
                }
                let r = ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]);
                if !r.iter().all(|v| v.is_finite()) {
                    return None;
                }
                rect = Some(r);
            }
            b"f" | b"F" | b"f*" if op.operands.is_empty() && white => {
                let crop = page.crop(doc);
                if !crop.iter().all(|v| v.is_finite()) || !contains(rect?, crop, 0.0) {
                    return None;
                }
                painted = true;
            }
            _ => return None,
        }
        if painted && stack.is_empty() {
            if ctm != Matrix::IDENTITY {
                return None;
            }
            let part = pieces.piece_of(op.span.end.checked_sub(1)?);
            let start = data.iter().take(part).try_fold(0usize, |n, b| n.checked_add(b.len().checked_add(1)?))?;
            let offset = op.span.end.checked_sub(start)?;
            let bytes = data.get(part)?.clone();
            bytes.get(..offset)?;
            return Some(Boundary { index: *indices.get(part)?, offset, bytes });
        }
    }
    None
}

pub(super) fn supported(doc: &Document, page: usize) -> bool {
    let Some(page) = page_list(doc).into_iter().nth(page) else { return false };
    let list = match page.dict.get(b"Contents").map(|r| doc.resolve(r)).as_deref() {
        Some(Object::Array(a)) => a.clone(),
        Some(Object::Stream(_)) => page.dict.get(b"Contents").cloned().into_iter().collect(),
        _ => return false,
    };
    boundary(doc, &page, &list).is_some()
}

/// Split just the stream containing the boundary, retaining its bytes in two fragments.
/// Do not keep a reference to the original: it could retain text after a later redaction and
/// full save. Removing the background leaves the fragments as ordinary page content.
pub(super) fn place(doc: &mut Document, page: &pdfcraft_model::Page, content: &[u8]) -> Result<bool, EditError> {
    let mut list = contents(doc, page)?;
    let Some(at) = boundary(doc, page, &list) else { return Ok(false) };
    let Some(original) = list.get(at.index).cloned().filter(|o| o.as_ref().is_some()) else { return Ok(false) };
    let resolved = doc.resolve(&original);
    let Object::Stream(stream) = &*resolved else { return Ok(false) };
    let (Some(prefix), Some(suffix)) = (at.bytes.get(..at.offset), at.bytes.get(at.offset..)) else { return Ok(false) };
    let mut insert = at.index.saturating_add(1);
    if !suffix.is_empty() {
        let tail = Object::Ref(doc.add(Object::Stream(Stream::flate(stream.dict.clone(), suffix))));
        let head = Object::Ref(doc.add(Object::Stream(Stream::flate(stream.dict.clone(), prefix))));
        if let Some(slot) = list.get_mut(at.index) {
            *slot = head;
        }
        list.insert(insert, tail);
    }
    // Repeated additions keep their chronological order above the same white fill.
    while list.get(insert).is_some_and(|o| stream_tag(doc, o).as_deref() == Some("Background")) {
        insert = insert.saturating_add(1);
    }
    let mark = Object::Ref(doc.add(Object::Stream(Stream::flate(tagged("Background"), content))));
    list.insert(insert, mark);
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list)))?;
    Ok(true)
}
