use super::blocks::{BlockStyle, TextBlock};
use super::fallback::MAX_TYPE3_GLYPHS;
use super::interp::{frame_corners, from_frame, to_frame};
use super::lines::{page_dict, text_lines};
use super::rewrite::rewrite_block_from;
use super::{DecodeBudget, Document, EditError, Metrics, Object, TextLine, TextSession, union};

/// An embedded sfnt source face, read only for the inline editor. Bytes stay in memory and
/// are never installed, copied into assets, or used as a font for another document.
#[derive(Clone, Debug)]
pub struct TextEditorFont {
    pub resource: String,
    pub face: String,
    pub bytes: Vec<u8>,
}

pub fn text_editor_fonts(doc: &Document, page: usize) -> Result<Vec<TextEditorFont>, EditError> {
    let p = page_dict(doc, page)?;
    let resources = p.dict.get(b"Resources").and_then(|o| doc.dict(o)).unwrap_or_default();
    let fonts = resources.get(b"Font").and_then(|o| doc.dict(o)).unwrap_or_default();
    let mut budget = 32_usize << 20;
    // Every program decoded counts, usable or not, so aliases of one program can't repeat the work.
    let mut decode = DecodeBudget::new(2 * budget, 0);
    let mut result = Vec::new();
    for (name, font) in fonts.iter().take(128) {
        let Some(font) = doc.dict(font) else { continue };
        let Some(bytes) = pdfcraft_fonts::pdf::embedded_sfnt_within(doc, &font, &mut decode) else { continue };
        if bytes.len() > budget {
            continue;
        }
        budget -= bytes.len();
        result.push(TextEditorFont {
            resource: String::from_utf8_lossy(name).into_owned(),
            face: Metrics::from_dict(doc, &font).base_font.rsplit('+').next().unwrap_or_default().to_owned(),
            bytes,
        });
    }
    // The approved full face follows all source subsets in the editor's font stack. It can
    // display newly inserted glyphs before Apply without changing the source text's face.
    let source_faces: Vec<_> = result.iter().map(|p| p.face.clone()).collect();
    for face in source_faces {
        for full in pdfcraft_fonts::embedding::matching_faces(&face) {
            if full.bytes.len() > budget || result.iter().any(|p| p.resource == format!("PCApproved{}", full.name())) {
                continue;
            }
            budget -= full.bytes.len();
            result.push(TextEditorFont { resource: format!("PCApproved{}", full.name()), face: face.clone(), bytes: full.bytes.to_vec() });
        }
    }
    Ok(result)
}

/// Load an explicit full face into this page's resources. Nothing on the page is replaced;
/// the new resource can complete matching source subsets or be chosen in a text style.
pub fn register_text_font(doc: &mut Document, page: usize, bytes: &[u8]) -> Result<String, EditError> {
    pdfcraft_fonts::embedding::validate_document_input(bytes).map_err(EditError::Invalid)?;
    let (_, postscript) = pdfcraft_fonts::embedding::names(bytes).map_err(EditError::Invalid)?;
    let p = page_dict(doc, page)?;
    let mut resources = p.dict.get(b"Resources").and_then(|o| doc.dict(o)).unwrap_or_default();
    let mut fonts = resources.get(b"Font").and_then(|o| doc.dict(o)).unwrap_or_default();
    let name = pdfcraft_fonts::embedding::embed(doc, &mut fonts, bytes, &postscript).map_err(EditError::Invalid)?;
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources)))?;
    Ok(name)
}

/// Read-only draft bounds, measured with the same PDF rewrite used by Apply/Save.
#[derive(Clone, Debug, PartialEq)]
pub struct TextEditLayout {
    pub source_rect: [f64; 4],
    pub draft_rect: Option<[f64; 4]>,
    pub overflow: bool,
}

pub fn text_edit_layout(doc: &Document, page: usize, block: usize, text: &str, style: &BlockStyle) -> Result<TextEditLayout, EditError> {
    TextSession::open(doc, page, block)?.layout(text, style)
}

/// Visible draft bounds in page space and in the paragraph's local frame.
fn draft_line_bounds(line: &TextLine, dir: (f64, f64)) -> ([f64; 4], [f64; 4]) {
    // Spaces typed after a row's last word are drawn but show nothing, so an upright row is
    // measured to the end of its last word: they don't make a draft overflow (#4199).
    let mut rect = line.rect;
    if line.upright() && !line.ink.is_empty() {
        if crate::bidi::rtl_base(&line.text) {
            rect[0] = rect[0].max(line.ink.iter().map(|i| i[0]).fold(f64::INFINITY, f64::min));
        } else {
            rect[2] = rect[2].min(line.ink.iter().map(|i| i[2]).fold(f64::NEG_INFINITY, f64::max));
        }
    }
    // Project the line's oriented corners, never its world-axis bounding box: rotating
    // that already enlarged box would report overflow for text that fits its frame.
    let corners = if line.upright() { frame_corners(rect) } else { frame_corners(line.frame_rect()).map(|p| from_frame(line.direction(), p)) };
    let local = corners
        .map(|p| to_frame(dir, p))
        .into_iter()
        .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |r, p| [r[0].min(p.0), r[1].min(p.1), r[2].max(p.0), r[3].max(p.1)]);
    (rect, local)
}

/// A draft of paragraph `source` (number `block` among the page's `lines`) laid out by the rewrite
/// Apply runs, on a copy of `doc`.
pub(crate) fn layout_draft(
    doc: &Document,
    page: usize,
    lines: &[TextLine],
    source: &TextBlock,
    block: usize,
    text: &str,
    style: &BlockStyle,
) -> Result<TextEditLayout, EditError> {
    let source_rect = source.editing_rect();
    let dir = source.direction();
    let mut target_rect = source.editing_frame();
    if let Some(width) = style.width {
        target_rect[2] = target_rect[0] + width;
    }
    if let Some([bottom, top]) = style.vertical_frame {
        target_rect[1] = bottom;
        target_rect[3] = top;
    }
    if let Some([dx, dy]) = style.offset {
        let (dx, dy) = to_frame(dir, (dx, dy));
        target_rect = [target_rect[0] + dx, target_rect[1] + dy, target_rect[2] + dx, target_rect[3] + dy];
    }
    let outside =
        |r: [f64; 4]| r[0] < target_rect[0] - 0.5 || r[1] < target_rect[1] - 0.5 || r[2] > target_rect[2] + 0.5 || r[3] > target_rect[3] + 0.5;
    if text == source.editor_text && *style == BlockStyle::default() {
        // Measure the existing runs without rewriting them, using the same visible bounds as
        // an edited draft. Preserved trailing spaces must not change the warning after Apply.
        let bounds = source
            .lines
            .iter()
            .filter_map(|index| lines.get(*index))
            .map(|line| draft_line_bounds(line, dir))
            .reduce(|(rect, frame), (r, f)| (union(rect, r), union(frame, f)));
        let (draft_rect, overflow) = bounds.map_or((None, false), |(rect, frame)| (Some(rect), outside(frame)));
        return Ok(TextEditLayout { source_rect, draft_rect, overflow });
    }
    // Subtract the untouched runs by value, rather than by operator offsets: a rewrite may
    // split BT/ET or add font resources. A multiset also handles coincident repeated text.
    let key =
        |line: &TextLine| (line.text.clone(), line.font.clone(), line.rect.map(f64::to_bits), line.size.to_bits(), line.color.map(f64::to_bits));
    let mut untouched = std::collections::BTreeMap::new();
    for (index, line) in lines.iter().enumerate() {
        if !source.lines.contains(&index) {
            *untouched.entry(key(line)).or_insert(0_usize) += 1;
        }
    }
    let mut draft = doc.clone();
    rewrite_block_from(&mut draft, page, lines.to_vec(), block, Some(text), style, MAX_TYPE3_GLYPHS)?;
    let mut rect: Option<[f64; 4]> = None;
    let mut frame: Option<[f64; 4]> = None;
    for line in text_lines(&draft, page)? {
        if let Some(count) = untouched.get_mut(&key(&line))
            && *count > 0
        {
            *count -= 1;
            continue;
        }
        let (r, local) = draft_line_bounds(&line, dir);
        rect = Some(rect.map_or(r, |previous| union(previous, r)));
        frame = Some(frame.map_or(local, |r| union(r, local)));
    }
    let overflow = frame.is_some_and(outside);
    Ok(TextEditLayout { source_rect, draft_rect: rect, overflow })
}
