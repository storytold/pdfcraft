//! Copy with formatting: the selected glyphs of a page as paragraphs of styled spans and an
//! HTML fragment ([`pdfcraft_export::clip_html`]) for the clipboard, next to the plain text.
//!
//! Glyphs come from the text layer (view space); style comes from the page's reading blocks,
//! which never mix styles, mapped into view space. A glyph takes the style of the block its
//! centre lies in; a glyph in no block is copied unstyled.

use pdfcraft_export::{ClipBuilder, ClipStyle, ClipText, clip_html};
use pdfcraft_render::{PageInfo, PageText};

use crate::EditError;
use crate::compare::{display_unit, weight_and_slant};

/// Block tests spent matching glyphs to blocks for one copy; past it the rest is unstyled.
const MAX_MATCH_STEPS: usize = 20_000_000;
/// How far outside a block's box (points) a glyph centre still belongs to it.
const SLACK: f32 = 0.5;

/// Selected text with its formatting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RichCopy {
    /// The paragraphs and spans.
    pub text: ClipText,
    /// The HTML fragment for the clipboard (empty when nothing was selected).
    pub html: String,
    /// Styles were left out: the document has no editor to read them, reading stopped at its
    /// limits on this page, or matching glyphs to blocks ran out of steps.
    pub styles_incomplete: bool,
    /// A cap left part of the selection out of `text` and `html`; the plain text has all of it.
    pub truncated: bool,
}

struct StyledBlock {
    rect: [f32; 4],
    style: ClipStyle,
}

impl crate::Document {
    /// Selected `glyphs` of `page` (indices into `text`, in copy order) with their formatting.
    /// Refused when the document does not allow copying.
    pub fn rich_copy(&self, page: usize, text: &PageText, glyphs: &[usize]) -> Result<RichCopy, EditError> {
        self.rich_copy_within(page, text, glyphs, MAX_MATCH_STEPS)
    }

    /// [`Self::rich_copy`] with `steps` block tests to match glyphs to blocks.
    fn rich_copy_within(&self, page: usize, text: &PageText, glyphs: &[usize], mut steps: usize) -> Result<RichCopy, EditError> {
        self.check_copying()?;
        let (blocks, mut incomplete) = self.styled_blocks(page);
        let mut nested = vec![None; blocks.len()];
        let mut b = ClipBuilder::new();
        // The previous glyph's line, block and box, and whether it was a hyphen.
        let mut prev: Option<(usize, Option<usize>, [f32; 4], bool)> = None;
        for &i in glyphs {
            let (Some(g), Some(&line)) = (text.glyphs.get(i), text.line_of.get(i)) else { continue };
            let hint = prev.and_then(|(_, b, _, _)| b).filter(|&k| !holds_smaller(&blocks, &mut nested, k, &mut steps));
            let found = block_at(&blocks, g.rect, hint, &mut steps).unwrap_or_else(|| {
                incomplete = true;
                None
            });
            let line = usize::try_from(line).unwrap_or(usize::MAX);
            if let Some((prev_line, prev_block, prev_rect, hyphen)) = prev {
                if line != prev_line {
                    let style = |k: Option<usize>| k.and_then(|k| blocks.get(k)).map(|s| &s.style);
                    if found.is_none() && prev_block.is_none() {
                        b.line_break();
                    } else if found == prev_block || wraps(style(prev_block), style(found), prev_rect, g.rect) {
                        // A paragraph wrapping: one space, none after a hyphen.
                        if !hyphen {
                            b.space();
                        }
                    } else {
                        b.paragraph();
                    }
                } else if text.space_before.get(i).copied().unwrap_or(false) {
                    b.space();
                }
            }
            let style = found.and_then(|k| blocks.get(k)).map(|s| &s.style);
            if !b.push(&g.text, style.unwrap_or(&ClipStyle::default())) {
                break;
            }
            prev = Some((line, found, g.rect, g.text.ends_with('-')));
        }
        Ok(finish(b, incomplete))
    }

    /// `page`'s reading blocks in view space with their style, and whether reading stopped at
    /// its limits.
    fn styled_blocks(&self, page: usize) -> (Vec<StyledBlock>, bool) {
        let Some(info) = self.info.pages.get(page) else { return (Vec::new(), false) };
        let Some(editor) = self.editor.as_ref() else { return (Vec::new(), true) };
        let Ok(reading) = pdfcraft_edit::reading_blocks_checked(&editor.cos, page) else { return (Vec::new(), true) };
        let unit = display_unit(info);
        let blocks = reading
            .blocks
            .into_iter()
            .filter(|b| !b.text.trim().is_empty())
            .map(|b| {
                let (bold, italic) = weight_and_slant(&b.base_font);
                let style = ClipStyle {
                    families: pdfcraft_export::font_family(&b.base_font),
                    size: b.size * unit,
                    bold: b.bold || bold,
                    italic: b.italic || italic,
                    color: b.color,
                };
                StyledBlock { rect: view_rect(info, b.rect), style }
            })
            .collect();
        (blocks, reading.truncated)
    }
}

/// A user-space box as a view-space box (points from the displayed page's top-left, y down).
fn view_rect(info: &PageInfo, r: [f64; 4]) -> [f32; 4] {
    let a = info.user_to_view(r[0] as f32, r[1] as f32);
    let b = info.user_to_view(r[2] as f32, r[3] as f32);
    [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
}

/// The copy `b` collected, with its HTML; `truncated` when a cap cut the text or the HTML.
fn finish(b: ClipBuilder, styles_incomplete: bool) -> RichCopy {
    let text = b.finish();
    let (html, cut) = clip_html(&text);
    RichCopy { html, truncated: text.truncated || cut, text, styles_incomplete }
}

/// Whether the next line starts inside the same paragraph although its first glyph is in another
/// block: blocks split where the style changes, so a style change at a line's end is still a
/// wrap when the size stays and the lines sit no further apart than their own height allows.
/// Two blocks of the same style, a change of size, or a gap of over half a line are paragraphs.
fn wraps(prev: Option<&ClipStyle>, next: Option<&ClipStyle>, prev_rect: [f32; 4], rect: [f32; 4]) -> bool {
    let (Some(prev), Some(next)) = (prev, next) else { return false };
    let diff = (prev.size - next.size).abs();
    if prev == next || diff.is_nan() || diff > 0.01 {
        return false;
    }
    let height = (prev_rect[3] - prev_rect[1]).max(rect[3] - rect[1]);
    // The next line starts just below the last one; a line far above it is another column.
    let gap = rect[1] - prev_rect[3];
    height > 0.0 && gap.is_finite() && gap >= -height / 2.0 && gap <= height / 2.0
}

/// Whether a block smaller than block `k` overlaps it, so a glyph inside `k` may belong to that
/// one instead. Worked out once per block (`nested` keeps it), at a step per block tested; when
/// the steps run out the answer is yes, so the full search decides.
fn holds_smaller(blocks: &[StyledBlock], nested: &mut [Option<bool>], k: usize, steps: &mut usize) -> bool {
    if let Some(Some(known)) = nested.get(k) {
        return *known;
    }
    let Some(outer) = blocks.get(k) else { return true };
    let area = |r: &[f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    let overlaps = |r: &[f32; 4]| r[0] < outer.rect[2] && r[2] > outer.rect[0] && r[1] < outer.rect[3] && r[3] > outer.rect[1];
    let mut found = false;
    for (j, b) in blocks.iter().enumerate() {
        if *steps == 0 {
            return true;
        }
        *steps -= 1;
        if j != k && overlaps(&b.rect) && area(&b.rect) < area(&outer.rect) {
            found = true;
            break;
        }
    }
    if let Some(slot) = nested.get_mut(k) {
        *slot = Some(found);
    }
    found
}

/// The block whose box holds the centre of `glyph` (view space): `hint` first (a block no
/// smaller one overlaps), then the smallest that holds it. Each block tested costs a step;
/// `None` when the steps ran out first.
fn block_at(blocks: &[StyledBlock], glyph: [f32; 4], hint: Option<usize>, steps: &mut usize) -> Option<Option<usize>> {
    let (x, y) = ((glyph[0] + glyph[2]) / 2.0, (glyph[1] + glyph[3]) / 2.0);
    let holds = |r: &[f32; 4]| x >= r[0] - SLACK && x <= r[2] + SLACK && y >= r[1] - SLACK && y <= r[3] + SLACK;
    let area = |r: &[f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    if let Some(k) = hint
        && *steps > 0
    {
        *steps -= 1;
        if blocks.get(k).is_some_and(|b| holds(&b.rect)) {
            return Some(Some(k));
        }
    }
    let mut best: Option<(usize, f32)> = None;
    for (k, b) in blocks.iter().enumerate() {
        if *steps == 0 {
            return None;
        }
        *steps -= 1;
        if holds(&b.rect) && best.is_none_or(|(_, a)| area(&b.rect) < a) {
            best = Some((k, area(&b.rect)));
        }
    }
    Some(best.map(|(k, _)| k))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_export::{ClipBuilder, ClipStyle};

    use crate::Session;

    /// A heading in bold, a body paragraph of two lines with HTML-special characters, and a red
    /// word on the body's first line.
    fn styled_page() -> Vec<u8> {
        let content = b"BT /F1 24 Tf 20 250 Td (Title) Tj ET \
            BT /F2 12 Tf 20 200 Td (Body text) Tj ET \
            BT 1 0 0 rg /F2 12 Tf 90 200 Td (red) Tj ET \
            BT /F2 12 Tf 20 186 Td (a<b & c) Tj ET";
        page_with(content)
    }

    /// A one-page PDF drawing `content` with `/F1` Helvetica-Bold and `/F2` Helvetica.
    fn page_with(content: &[u8]) -> Vec<u8> {
        pdfcraft_testkit::Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 300] >>")
            .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>")
            .stream("", content)
            .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>")
            .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
            .build()
    }

    fn page_text(s: &Session, id: crate::DocId) -> Arc<pdfcraft_render::PageText> {
        let doc = s.get(id).unwrap();
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), Default::default());
        let request = pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() };
        r.render(request).text.unwrap()
    }

    #[test]
    fn copy_keeps_headings_body_and_colour_as_html() {
        let mut s = Session::new();
        let id = s.open("styled.pdf", None, Arc::new(styled_page()), None).unwrap();
        let text = page_text(&s, id);
        let all: Vec<usize> = (0..text.glyphs.len()).collect();
        let copy = s.get(id).unwrap().rich_copy(0, &text, &all).unwrap();
        let html = &copy.html;
        assert!(!copy.styles_incomplete);
        assert!(
            html.starts_with("<p style=\"margin:0\"><span style=\"font-family:'Helvetica';font-size:24pt\"><strong>Title</strong></span></p>\n"),
            "{html}"
        );
        assert!(html.contains("font-size:12pt\">Body text "), "{html}");
        assert!(html.contains("font-size:12pt;color:#FF0000\">red</span>"), "{html}");
        assert!(html.contains("a&lt;b &amp; c"), "{html}");
        assert!(!html.contains("<b &"), "{html}");
        assert_eq!(copy.text.paragraphs.len(), html.matches("<p ").count());
        assert!(copy.text.paragraphs.len() >= 2, "{html}");
        // The heading and the body are separate paragraphs; the red word shares the body's.
        let body = copy.text.paragraphs.iter().find(|p| p.iter().any(|s| s.text.contains("Body"))).unwrap();
        assert!(body.iter().any(|s| s.text == "red" && s.style.color == [1.0, 0.0, 0.0]), "{body:?}");

        // A part of the selection: only those glyphs.
        let part = s.get(id).unwrap().rich_copy(0, &text, &all[..5]).unwrap();
        assert_eq!(part.text.paragraphs.len(), 1);
        assert!(part.html.contains("<strong>Title</strong>"), "{}", part.html);
        // Nothing selected, and indices past the text: an empty copy, not a panic.
        let none = s.get(id).unwrap().rich_copy(0, &text, &[]).unwrap();
        assert!(none.html.is_empty() && none.text.paragraphs.is_empty());
        let past = s.get(id).unwrap().rich_copy(7, &text, &[usize::MAX, text.glyphs.len()]).unwrap();
        assert!(past.html.is_empty());
    }

    #[test]
    fn running_out_of_match_steps_copies_the_rest_unstyled() {
        let mut s = Session::new();
        let id = s.open("styled.pdf", None, Arc::new(styled_page()), None).unwrap();
        let text = page_text(&s, id);
        let all: Vec<usize> = (0..text.glyphs.len()).collect();
        let doc = s.get(id).unwrap();
        let full = doc.rich_copy(0, &text, &all).unwrap();
        // Enough for the first glyphs only: the text is all there, the rest has no style.
        let short = doc.rich_copy_within(0, &text, &all, 8).unwrap();
        assert!(short.styles_incomplete);
        let plain = |c: &super::RichCopy| c.text.paragraphs.iter().flatten().map(|s| s.text.clone()).collect::<String>();
        assert_eq!(plain(&short).replace([' ', '\n'], ""), plain(&full).replace([' ', '\n'], ""));
        let last = short.text.paragraphs.last().and_then(|p| p.last()).unwrap();
        assert_eq!(last.style, pdfcraft_export::ClipStyle::default(), "{last:?}");
        assert!(short.html.starts_with("<p style=\"margin:0\"><span style=\"font-family:'Helvetica';font-size:24pt\"><strong>T"), "{}", short.html);
        // No steps at all: nothing styled, still no panic.
        let none = doc.rich_copy_within(0, &text, &all, 0).unwrap();
        assert!(none.styles_incomplete && !none.html.contains("font-size"), "{}", none.html);
        // A page without blocks to match needs no steps.
        let page_less = doc.rich_copy_within(5, &text, &all, 0).unwrap();
        assert!(!page_less.styles_incomplete);
    }

    fn copy_all(s: &Session, id: crate::DocId) -> super::RichCopy {
        let text = page_text(s, id);
        let all: Vec<usize> = (0..text.glyphs.len()).collect();
        s.get(id).unwrap().rich_copy(0, &text, &all).unwrap()
    }

    #[test]
    fn a_style_change_at_a_line_end_stays_in_its_paragraph() {
        let content = b"BT /F2 12 Tf 20 200 Td (plain then) Tj ET \
            BT /F1 12 Tf 85 200 Td (bold) Tj ET \
            BT /F2 12 Tf 20 186 Td (continues here) Tj ET \
            BT /F2 12 Tf 20 120 Td (Another paragraph) Tj ET";
        let mut s = Session::new();
        let id = s.open("wrap.pdf", None, Arc::new(page_with(content)), None).unwrap();
        let copy = copy_all(&s, id);
        let text = |p: &Vec<pdfcraft_export::ClipSpan>| p.iter().map(|s| s.text.as_str()).collect::<String>();
        let paragraphs: Vec<String> = copy.text.paragraphs.iter().map(text).collect();
        assert!(paragraphs.iter().any(|p| p.contains("bold") && p.contains("continues")), "{paragraphs:?}");
        assert!(paragraphs.iter().any(|p| p.contains("Another") && !p.contains("continues")), "a wide gap is still a paragraph: {paragraphs:?}");
    }

    #[test]
    fn a_style_change_into_the_next_column_starts_a_paragraph() {
        let content = b"BT /F2 12 Tf 20 200 Td (left top) Tj ET \
            BT /F2 12 Tf 20 186 Td (left end) Tj ET \
            BT /F1 12 Tf 170 200 Td (Right head) Tj ET \
            BT /F2 12 Tf 170 186 Td (right body) Tj ET";
        let mut s = Session::new();
        let id = s.open("columns.pdf", None, Arc::new(page_with(content)), None).unwrap();
        let copy = copy_all(&s, id);
        let text = |p: &Vec<pdfcraft_export::ClipSpan>| p.iter().map(|s| s.text.as_str()).collect::<String>();
        let paragraphs: Vec<String> = copy.text.paragraphs.iter().map(text).collect();
        assert!(paragraphs.iter().any(|p| p.contains("Right")), "{paragraphs:?}");
        assert!(!paragraphs.iter().any(|p| p.contains("left end") && p.contains("Right")), "{paragraphs:?}");
    }

    #[test]
    fn a_styled_word_inside_a_paragraph_keeps_its_style() {
        let content = b"BT /F2 12 Tf 20 200 Td (first row of the text) Tj ET \
            BT /F2 12 Tf 20 186 Td (second) Tj ET \
            BT /F1 12 Tf 62 186 Td (bold) Tj ET \
            BT /F2 12 Tf 92 186 Td (row) Tj ET";
        let mut s = Session::new();
        let id = s.open("nested.pdf", None, Arc::new(page_with(content)), None).unwrap();
        let copy = copy_all(&s, id);
        let spans: Vec<_> = copy.text.paragraphs.iter().flatten().collect();
        assert!(spans.iter().any(|s| s.text.contains("bold") && s.style.bold), "{spans:?}");
        assert!(spans.iter().any(|s| s.text.contains("second") && !s.style.bold), "{spans:?}");
    }

    #[test]
    fn a_capped_copy_is_marked_truncated() {
        let (plain, bold) = (ClipStyle::default(), ClipStyle { bold: true, ..ClipStyle::default() });
        let mut b = ClipBuilder::new();
        for k in 0..=pdfcraft_export::MAX_CLIP_SPANS {
            b.push("x", if k % 2 == 0 { &plain } else { &bold });
        }
        let copy = super::finish(b, false);
        assert!(copy.truncated && copy.text.truncated);
        let mut b = ClipBuilder::new();
        b.push("x", &plain);
        assert!(!super::finish(b, false).truncated);
    }

    #[test]
    fn without_an_editor_the_styles_are_incomplete() {
        let mut s = Session::new();
        let id = s.open("styled.pdf", None, Arc::new(styled_page()), None).unwrap();
        let text = page_text(&s, id);
        let all: Vec<usize> = (0..text.glyphs.len()).collect();
        s.doc_mut(id).unwrap().editor = None;
        let copy = s.get(id).unwrap().rich_copy(0, &text, &all).unwrap();
        assert!(copy.styles_incomplete && !copy.html.contains("font-size"), "{}", copy.html);
        assert!(!copy.truncated);
    }

    #[test]
    fn copy_with_formatting_is_refused_when_copying_is_withheld() {
        let mut cos = pdfcraft_cos::Document::open(Arc::new(styled_page())).unwrap();
        cos.set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: "",
            owner_password: "owner",
            permissions: 0b0100, // print only
            encrypt_metadata: true,
            seed: [7; 32],
        })
        .unwrap();
        let bytes = Arc::new(pdfcraft_cos::write_full(&cos, &Default::default()).unwrap());
        let mut s = Session::new();
        let id = s.open("locked.pdf", None, bytes, None).unwrap();
        let text = pdfcraft_render::PageText { glyphs: Vec::new(), line_of: Vec::new(), space_before: Vec::new(), truncated: false };
        let err = s.get(id).unwrap().rich_copy(0, &text, &[]).unwrap_err();
        assert!(matches!(err, crate::EditError::NotPermitted(_)), "{err:?}");
    }
}
