use super::blocks::{BlockStyle, TextBlock};
use super::fallback::MAX_TYPE3_GLYPHS;
use super::lines::{page_dict, text_lines};
use super::rewrite::rewrite_block_from;
use super::{DecodeBudget, Dict, Document, EditError, Object, TextLine, TextSession};

/// An embedded source font or an approved bundled document-font choice.
/// Source bytes stay in memory and are never installed or copied into another document.
#[derive(Clone, Debug)]
pub struct TextEditorFont {
    pub resource: String,
    pub face: String,
    pub bytes: Vec<u8>,
    /// A selectable replacement, rather than a source/matching preview face. Merely listing
    /// this choice must not reload an open editor's font atlas.
    pub choice_only: bool,
}

/// Reserved document-font choice for explicitly replacing text with the bundled Thai face.
/// This is a selector, never a font resource written into a PDF.
const BUILTIN_THAI_FONT: &str = "builtin:thai";

pub(super) fn builtin_thai_choice(fonts: &Dict) -> String {
    let mut choice = BUILTIN_THAI_FONT.to_owned();
    while fonts.contains(choice.as_bytes()) {
        choice.push('_');
    }
    choice
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
            face: pdfcraft_fonts::pdf::base_font_name(doc, &font).rsplit('+').next().unwrap_or_default().to_owned(),
            bytes,
            choice_only: false,
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
            result.push(TextEditorFont {
                resource: format!("PCApproved{}", full.name()),
                face: face.clone(),
                bytes: full.bytes.to_vec(),
                choice_only: false,
            });
        }
    }
    // A missing source family must not make the bundled Thai replacement undiscoverable.
    // Offering it does not change the source; callers explicitly select this choice to replace it.
    if let Some(thai) = pdfcraft_fonts::document_thai_font()
        && thai.bytes.len() <= budget
    {
        result.push(TextEditorFont {
            resource: builtin_thai_choice(&fonts),
            face: thai.family.into(),
            bytes: thai.bytes.to_vec(),
            choice_only: true,
        });
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
    pub rows: Vec<super::caret::DraftRow>,
}

pub fn text_edit_layout(doc: &Document, page: usize, block: usize, text: &str, style: &BlockStyle) -> Result<TextEditLayout, EditError> {
    TextSession::open(doc, page, block)?.layout(text, style)
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
    if text == source.editor_text && *style == BlockStyle::default() {
        let selected: Vec<_> = source.lines.iter().filter_map(|i| lines.get(*i).cloned()).collect();
        let rows = super::caret::draft_rows(text, &selected, source.rect, source.size * 1.5);
        return Ok(TextEditLayout { source_rect: source.rect, draft_rect: Some(source.rect), overflow: false, rows });
    }
    // Subtract the untouched runs by value, rather than by operator offsets: a rewrite may
    // split BT/ET or add font resources. A multiset also handles coincident repeated text.
    let key = |line: &TextLine| (line.text.clone(), line.font.clone(), line.size.to_bits(), line.color.map(f64::to_bits));
    // Each key keeps its rectangles in page order, with a cursor past the ones already matched:
    // the rewrite draws untouched runs in the same order, so matching usually costs one comparison.
    let mut untouched: std::collections::BTreeMap<_, (usize, Vec<Option<[f64; 4]>>)> = std::collections::BTreeMap::new();
    for (index, line) in lines.iter().enumerate() {
        if !source.lines.contains(&index) {
            untouched.entry(key(line)).or_default().1.push(Some(line.rect));
        }
    }
    let mut draft = doc.clone();
    rewrite_block_from(&mut draft, page, lines.to_vec(), block, Some(text), style, MAX_TYPE3_GLYPHS)?;
    let mut rect: Option<[f64; 4]> = None;
    let mut written = Vec::new();
    // Repeated identical strings at many different positions must not make matching quadratic
    // without a bound. Once the bound is spent, an unmatched run may be untouched page text, so
    // the draft's bounds are unknown: report none, rather than unrelated text as the draft.
    let mut comparisons = 1_000_000_usize;
    for line in text_lines(&draft, page)? {
        if let Some((start, rects)) = untouched.get_mut(&key(&line)) {
            let mut found = None;
            for (index, slot) in rects.iter().enumerate().skip(*start) {
                let Some(left) = comparisons.checked_sub(1) else { break };
                comparisons = left;
                let Some(r) = slot else { continue };
                // Restoring a text matrix during rewriting rounds its operands. Bitwise
                // comparison mistakes unaffected paragraphs for draft text, invalidating all
                // caret rows. This tolerance is far below a visible displacement in PDF points.
                if r.iter().zip(line.rect).all(|(a, b)| (a - b).abs() <= 0.001) {
                    found = Some(index);
                    break;
                }
            }
            if let Some(index) = found {
                if let Some(slot) = rects.get_mut(index) {
                    *slot = None;
                }
                while rects.get(*start).is_some_and(Option::is_none) {
                    *start += 1;
                }
                continue;
            }
            if comparisons == 0 {
                return Ok(TextEditLayout { source_rect: source.rect, draft_rect: None, overflow: false, rows: Vec::new() });
            }
        }
        // Spaces typed after a row's last word are drawn but show nothing, so an upright row is
        // measured to the end of its last word: they don't make a draft overflow (#4199).
        let mut r = line.rect;
        if line.upright() && !line.ink.is_empty() {
            if crate::bidi::rtl_base(&line.text) {
                r[0] = r[0].max(line.ink.iter().map(|i| i[0]).fold(f64::INFINITY, f64::min));
            } else {
                r[2] = r[2].min(line.ink.iter().map(|i| i[2]).fold(f64::NEG_INFINITY, f64::max));
            }
        }
        rect = Some(rect.map_or(r, |a| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])]));
        written.push(line);
    }
    let overflow = rect
        .is_some_and(|r| r[0] < source.rect[0] - 0.5 || r[1] < source.rect[1] - 0.5 || r[2] > source.rect[2] + 0.5 || r[3] > source.rect[3] + 0.5);
    let leading = style.line_spacing.unwrap_or(1.5) * style.size.unwrap_or(source.size);
    let rows = super::caret::draft_rows(text, &written, source.rect, leading);
    Ok(TextEditLayout { source_rect: source.rect, draft_rect: rect, overflow, rows })
}
