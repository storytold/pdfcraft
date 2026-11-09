//! Copy with formatting: the selected glyphs of a page as paragraphs of styled spans and an
//! HTML fragment ([`pdfcraft_export::clip_html`]) for the clipboard, next to the plain text.
//!
//! Glyphs come from the text layer (view space); style comes from the page's reading blocks,
//! which never mix styles, mapped into view space. A glyph takes the style of the block its
//! centre lies in; a glyph in no block is copied unstyled.
//!
//! Copy with formatting also copies text the strict text layer can't name, from the page's
//! recovered layer ([`pdfcraft_render::RecoveredText`], read in the same pass as the strict
//! layer by a `RequestKind::RecoveredText` request): glyphs whose fonts say which character
//! they draw in a way ISO 32000-2 §9.10.2 doesn't define. Find, plain copy and text extraction
//! stay strict.

use pdfcraft_edit::strip_subset_tag;
use pdfcraft_export::{ClipBuilder, ClipStyle, ClipText, clip_html};
use pdfcraft_render::{PageInfo, PageText, RecoveredText};

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
    /// Some copied glyphs were read by a recovery rule: the strict plain text of the selection
    /// lacks them (or reads them otherwise), so the plain alternative written with the HTML is
    /// [`Self::plain_text`].
    pub recovered: bool,
}

impl RichCopy {
    /// The copied text without its formatting: paragraphs on lines of their own.
    pub fn plain_text(&self) -> String {
        let paragraphs: Vec<String> = self.text.paragraphs.iter().map(|p| p.iter().map(|s| s.text.as_str()).collect()).collect();
        paragraphs.join("\n")
    }
}

struct StyledBlock {
    rect: [f32; 4],
    style: ClipStyle,
    /// The block's `/BaseFont` without its subset tag: a recovered glyph drawn in the same font
    /// and size belongs to its run.
    font: String,
}

/// Sizes (points) this close are one size: the reading blocks and the drawn glyphs work a size
/// out in different arithmetic (`11.955` against `11.954999923706055`).
const SIZE_NOISE_PT: f64 = 0.025;
/// Colour channels this close are one colour: a drawn colour comes through 8 bits per channel
/// (0.5 reads as 128/255), the reading blocks' colour does not.
const COLOUR_NOISE: f64 = 1.0 / 255.0;

/// Whether two styles differ only by numeric noise between the reading blocks and the drawn
/// glyphs (size and colour within [`SIZE_NOISE_PT`] and [`COLOUR_NOISE`]); families, bold and
/// italic compare exactly, so every real difference stays its own span. Used only between a
/// recovered glyph styled as drawn and the span before it: strict pieces keep Copy's spans.
fn same_style(a: &ClipStyle, b: &ClipStyle) -> bool {
    a.families == b.families
        && a.bold == b.bold
        && a.italic == b.italic
        && same_size(a.size, b.size)
        && a.color.iter().zip(b.color).all(|(x, y)| (x - y).abs() <= COLOUR_NOISE)
}

/// Whether two sizes are one size, up to numeric noise.
fn same_size(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= SIZE_NOISE_PT
}

/// Bold and italic as a font's name says them, for a recovered glyph styled as drawn: style
/// words (`Arial-BoldItalicMT`) or a typesetter's `.B`, `.I` or `.BI` suffix
/// (`AdvTT2cba4af3.B`).
fn named_style(base_font: &str) -> (bool, bool) {
    let (bold, italic) = weight_and_slant(base_font);
    let suffix = base_font.rsplit_once('.').map(|(_, s)| s).unwrap_or_default();
    (bold || matches!(suffix, "B" | "BI"), italic || matches!(suffix, "I" | "BI"))
}

impl crate::Document {
    /// Selected `glyphs` of `page` (indices into `text`, in copy order) with their formatting, as
    /// Copy puts them on the clipboard: each glyph takes the style of the reading block it lies
    /// in. Refused when the document does not allow copying.
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

    /// Selected `glyphs` of `page` with their formatting, as Copy with formatting puts them on
    /// the clipboard: indices into `text`, which is `layer`'s text where the page has a
    /// recovered layer (what a `RequestKind::RecoveredText` request reads), whose recovered
    /// glyphs then read as their recovered text. It is [`Self::rich_copy`]'s copy with the
    /// recovered glyphs styled to blend in: every strict glyph keeps exactly the style Copy gives
    /// it (a selection with nothing recovered is [`Self::rich_copy`] of the same glyphs, in the
    /// layer's order: callers wanting Copy's copy byte for byte select the strict glyphs on the
    /// strict layer and call [`Self::rich_copy`]); a recovered glyph
    /// drawn in a reading block's run (its font and size, in the block or beside it on its line)
    /// takes that block's style, colour included; any other takes its style as drawn, joining the
    /// span before it when the two differ only by numeric noise ([`same_style`]). Refused when
    /// the document does not allow copying.
    pub fn rich_copy_formatted(&self, page: usize, text: &PageText, glyphs: &[usize], layer: Option<&RecoveredText>) -> Result<RichCopy, EditError> {
        let mut copy = self.rich_copy_formatted_within(page, text, glyphs, MAX_MATCH_STEPS, layer)?;
        copy.recovered = layer.is_some_and(|l| l.any_recovered(glyphs));
        Ok(copy)
    }

    /// [`Self::rich_copy_formatted`] of glyphs of `page`'s recovered layer `layer`.
    pub fn rich_copy_recovered(&self, page: usize, layer: &RecoveredText, glyphs: &[usize]) -> Result<RichCopy, EditError> {
        self.rich_copy_formatted(page, &layer.text, glyphs, Some(layer))
    }

    /// [`Self::rich_copy_formatted`] with `steps` block tests to match glyphs to blocks.
    fn rich_copy_formatted_within(
        &self,
        page: usize,
        text: &PageText,
        glyphs: &[usize],
        mut steps: usize,
        layer: Option<&RecoveredText>,
    ) -> Result<RichCopy, EditError> {
        // Nothing recovered selected: exactly Copy's copy.
        if !layer.is_some_and(|l| l.any_recovered(glyphs)) {
            return self.rich_copy_within(page, text, glyphs, steps);
        }
        self.check_copying()?;
        let (blocks, mut incomplete) = self.styled_blocks(page);
        let mut nested = vec![None; blocks.len()];
        let mut b = ClipBuilder::new();
        // The style of the last piece pushed: a recovered glyph styled as drawn whose style
        // differs from it only by numeric noise takes it, so noise never splits a span there.
        // Strict glyphs always keep their block's style, exactly as Copy gives it.
        let mut last_style: Option<ClipStyle> = None;
        // The previous glyph's line, block and box, and whether it was a hyphen.
        let mut prev: Option<(usize, Option<usize>, [f32; 4], bool)> = None;
        for &i in glyphs {
            let (Some(g), Some(&line)) = (text.glyphs.get(i), text.line_of.get(i)) else { continue };
            // A glyph past the copy's caps isn't copied, so its match can't leave the styles
            // incomplete.
            let incomplete_before = incomplete;
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
            let block = found.and_then(|k| blocks.get(k));
            let own;
            let mut as_drawn = false;
            let style = match (layer.and_then(|l| l.style_of(i)), block) {
                // A recovered glyph drawn in its block's font and size is in the block's run and
                // takes the block's style, colour included: the block reads a CMYK or spot colour
                // its own way, the drawn colour comes through the colour space.
                (Some(d), Some(block)) if in_run(d, block) => &block.style,
                (Some(d), block) => {
                    // Outside every block, one beside it on its line in its font and size runs
                    // on to it (a word of recovered glyphs after the strict ones of its font);
                    // past the match steps that is left untested, and the styles incomplete.
                    let next_to = match block {
                        Some(_) => None,
                        None => beside(&blocks, d, g.rect, &mut steps).unwrap_or_else(|| {
                            incomplete = true;
                            None
                        }),
                    };
                    match next_to {
                        Some(block) => &block.style,
                        // Otherwise its style as drawn (a pattern fill copies as black).
                        None => {
                            own = drawn_style(d);
                            as_drawn = true;
                            &own
                        }
                    }
                }
                (None, Some(block)) => &block.style,
                (None, None) => &ClipStyle::default(),
            };
            let snap = as_drawn && last_style.as_ref().is_some_and(|last| same_style(last, style));
            let style = match &last_style {
                Some(last) if snap => last,
                _ => style,
            };
            if !b.push(&g.text, style) {
                incomplete = incomplete_before;
                break;
            }
            if !snap {
                last_style = Some(style.clone());
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
                StyledBlock { rect: view_rect(info, b.rect), style, font: strip_subset_tag(&b.base_font).to_owned() }
            })
            .collect();
        (blocks, reading.truncated)
    }
}

/// Whether a recovered glyph drawn as `d` is in `block`'s run: the same font and size.
fn in_run(d: &pdfcraft_render::GlyphStyle, block: &StyledBlock) -> bool {
    d.font.as_deref().is_some_and(|f| strip_subset_tag(f) == block.font) && same_size(f64::from(d.size), block.style.size)
}

/// The nearest block on the line of a recovered glyph (box `rect`, drawn as `d`) whose run it
/// is in, for a glyph that lies in no block, if any; at a step per block tested, and `None` when
/// the steps run out.
fn beside<'b>(blocks: &'b [StyledBlock], d: &pdfcraft_render::GlyphStyle, rect: [f32; 4], steps: &mut usize) -> Option<Option<&'b StyledBlock>> {
    let cy = (rect[1] + rect[3]) / 2.0;
    let mut best: Option<(f32, &StyledBlock)> = None;
    for block in blocks {
        *steps = steps.checked_sub(1)?;
        if !(block.rect[1] - SLACK..=block.rect[3] + SLACK).contains(&cy) || !in_run(d, block) {
            continue;
        }
        let gap = (block.rect[0] - rect[2]).max(rect[0] - block.rect[2]).max(0.0);
        if best.is_none_or(|(g, _)| gap < g) {
            best = Some((gap, block));
        }
    }
    Some(best.map(|(_, block)| block))
}

/// A recovered glyph's style as drawn ([`pdfcraft_render::GlyphStyle`]): the family; bold and
/// italic from the font's flags, its name (`Arial-BoldItalicMT`) or a typesetter's `.B`, `.I`
/// or `.BI` suffix (`AdvTT2cba4af3.B`); its size in view-space points and its colour.
fn drawn_style(s: &pdfcraft_render::GlyphStyle) -> ClipStyle {
    let font = s.font.as_deref().unwrap_or_default();
    let (named_bold, named_italic) = named_style(font);
    ClipStyle {
        families: if font.is_empty() { Vec::new() } else { pdfcraft_export::font_family(font) },
        size: f64::from(s.size),
        bold: s.bold || named_bold,
        italic: s.italic || named_italic,
        color: s.color.map(|c| f64::from(c) / 255.0),
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
    RichCopy { html, truncated: text.truncated || cut, text, styles_incomplete, recovered: false }
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

    /// Page 0's recovered layer, as Copy with formatting reads it: `None` when no recovery rule
    /// read any glyph.
    fn recovered_layer(s: &Session, id: crate::DocId) -> Option<Arc<pdfcraft_render::RecoveredText>> {
        let doc = s.get(id).unwrap();
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), Default::default());
        let request = pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::RecoveredText, ..Default::default() };
        r.render(request).recovered
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
        let err = s.get(id).unwrap().rich_copy_recovered(0, &Default::default(), &[]).unwrap_err();
        assert!(matches!(err, crate::EditError::NotPermitted(_)), "{err:?}");
    }

    /// A one-page PDF: `Hi` in Helvetica, the codes 1 to n of a font whose `/Differences` from
    /// code 1 are `names` (glyph names that number Latin-1 characters, no `/ToUnicode`) on the
    /// same line, and `end` in Helvetica below; `strict_words: false` leaves the Helvetica words
    /// out.
    fn decimal_name_page(names: &str, strict_words: bool) -> Vec<u8> {
        decimal_name_page_in(names, strict_words, "AdvTTSynthetic")
    }

    /// [`decimal_name_page`] with the font named `base`.
    fn decimal_name_page_in(names: &str, strict_words: bool, base: &str) -> Vec<u8> {
        const CFF: &[u8] = include_bytes!("../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");
        let n = names.split_whitespace().count();
        let codes: String = (1..=n).map(|c| format!("{c:02x}")).collect();
        let mut content = format!("BT /F3 20 Tf 60 250 Td <{codes}> Tj ET");
        if strict_words {
            content = format!("BT /F1 20 Tf 20 250 Td (Hi) Tj ET {content} BT /F1 20 Tf 20 200 Td (end) Tj ET");
        }
        let widths = vec!["600"; n].join(" ");
        pdfcraft_testkit::Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 300] >>")
            .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F3 6 0 R >> >> >>")
            .stream("", content)
            .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
            .obj(format!("<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+{base} /FirstChar 1 /LastChar {n} /Widths [{widths}] /FontDescriptor 7 0 R /Encoding << /Type /Encoding /Differences [1 {names}] >> >>"))
            .obj(format!("<< /Type /FontDescriptor /FontName /ABCDEF+{base} /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 8 0 R >>"))
            .stream("/Subtype /Type1C", CFF)
            .build()
    }

    const DECIMAL_NAMES: &str = "/C83 /C111 /C99 /C246 /c105 /MT97 /C0108 /C32 /C174";

    /// A page whose only text is in a Type 0 font over a TrueType subset that kept its original
    /// glyph ids, with no `/ToUnicode`, `cmap` or glyph names: the glyph ids of `text` in the
    /// standard Macintosh glyph order.
    fn mac_order_page(text: &str) -> Vec<u8> {
        mac_order_page_styled(text, "ArialMT", "")
    }

    /// [`mac_order_page`] in font `base`, with `paint` (`1 0 0 rg`) before the text.
    fn mac_order_page_styled(text: &str, base: &str, paint: &str) -> Vec<u8> {
        use pdfcraft_testkit::truetype::{Cmap, mac_gid, subset};
        let gids: Vec<u16> = text.chars().map(mac_gid).collect();
        let shown: String = gids.iter().map(|g| format!("{g:04X}")).collect();
        pdfcraft_testkit::Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 300] >>")
            .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>")
            .stream("", format!("BT {paint} /F1 20 Tf 20 250 Td <{shown}> Tj ET"))
            .obj(format!("<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+{base} /Encoding /Identity-H /DescendantFonts [6 0 R] >>"))
            .obj(format!("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /ABCDEF+{base} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /CIDToGIDMap /Identity /DW 600 /FontDescriptor 7 0 R >>"))
            .obj(format!("<< /Type /FontDescriptor /FontName /ABCDEF+{base} /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 8 0 R >>"))
            .stream("", subset(300, &gids, Cmap::None, None))
            .build()
    }

    /// Copy with formatting copies the page's recovered layer: a selection from `Hi` to `end`
    /// also copies the glyphs of a font that names them by decimal Latin-1 number, in the HTML
    /// and its plain alternative, while the strict layer (plain copy, Find) reads only `Hi` and
    /// `end`, and the strict copy of the strict layer is unchanged.
    #[test]
    fn copy_with_formatting_copies_the_recovered_layer() {
        let mut s = Session::new();
        let id = s.open("advtt.pdf", None, Arc::new(decimal_name_page(DECIMAL_NAMES, true)), None).unwrap();
        let text = page_text(&s, id);
        assert_eq!(text.plain_text().split_whitespace().collect::<Vec<_>>(), ["Hi", "end"]);
        assert!(text.find("Soc").is_empty());
        let doc = s.get(id).unwrap();
        let strict = doc.rich_copy(0, &text, &(0..text.glyphs.len()).collect::<Vec<_>>()).unwrap();
        assert!(!strict.recovered && !strict.html.contains("Soc"), "{}", strict.html);
        let layer = recovered_layer(&s, id).expect("recovered layer");
        let all: Vec<usize> = (0..layer.text.glyphs.len()).collect();
        let copy = doc.rich_copy_recovered(0, &layer, &all).unwrap();
        assert!(copy.recovered);
        assert!(copy.html.contains("Soc\u{f6}ial \u{ae}"), "{}", copy.html);
        let plain = copy.plain_text();
        assert!(plain.starts_with("Hi") && plain.contains("Soc\u{f6}ial \u{ae}") && plain.ends_with("end"), "{plain:?}");
        // `Hi` alone: nothing recovered is selected.
        let hi = doc.rich_copy_recovered(0, &layer, &all[..2]).unwrap();
        assert!(!hi.recovered && hi.plain_text() == "Hi", "{:?}", hi.plain_text());
    }

    /// A page whose only text is recovered (rule K: glyph ids in the standard Macintosh order)
    /// has no strict text, and its recovered layer copies.
    #[test]
    fn a_page_of_recovered_text_copies_from_the_recovered_layer() {
        let mut s = Session::new();
        let id = s.open("form.pdf", None, Arc::new(mac_order_page("Articles of Amendment")), None).unwrap();
        assert!(page_text(&s, id).glyphs.is_empty());
        let doc = s.get(id).unwrap();
        let layer = recovered_layer(&s, id).expect("recovered layer");
        let all: Vec<usize> = (0..layer.text.glyphs.len()).collect();
        let copy = doc.rich_copy_recovered(0, &layer, &all).unwrap();
        assert!(copy.recovered);
        assert_eq!(copy.plain_text(), "Articles of Amendment");
        assert!(copy.html.contains("Articles of Amendment"), "{}", copy.html);
    }

    /// Recovered glyphs keep their formatting although the editor reads no block around them:
    /// the family, bold and italic from the font, the size and the colour as drawn.
    #[test]
    fn recovered_text_keeps_its_formatting() {
        let mut s = Session::new();
        let pdf = mac_order_page_styled("Articles of Amendment", "Arial-BoldItalicMT", "1 0 0 rg");
        let id = s.open("form.pdf", None, Arc::new(pdf), None).unwrap();
        let doc = s.get(id).unwrap();
        let layer = recovered_layer(&s, id).expect("recovered layer");
        let all: Vec<usize> = (0..layer.text.glyphs.len()).collect();
        let copy = doc.rich_copy_recovered(0, &layer, &all).unwrap();
        let span = copy.text.paragraphs.first().and_then(|p| p.first()).expect("a span");
        assert_eq!(span.text, "Articles of Amendment");
        assert!(span.style.bold && span.style.italic, "{:?}", span.style);
        assert!((span.style.size - 20.0).abs() < 0.5, "{:?}", span.style);
        assert_eq!(span.style.color, [1.0, 0.0, 0.0]);
        assert!(span.style.families.iter().any(|f| f == "Arial"), "{:?}", span.style.families);
        assert!(copy.html.contains("<strong>") && copy.html.contains("color:#FF0000"), "{}", copy.html);
        // A typesetter's `.B` suffix marks a bold face too.
        let bold = decimal_name_page_in("/C83 /C111 /C99", false, "AdvTTSynthetic.B");
        let id = s.open("advtt.pdf", None, Arc::new(bold), None).unwrap();
        let doc = s.get(id).unwrap();
        let layer = recovered_layer(&s, id).expect("recovered layer");
        let copy = doc.rich_copy_recovered(0, &layer, &[0, 1, 2]).unwrap();
        assert!(copy.html.contains("<strong>Soc"), "{}", copy.html);
    }

    /// One font for [`mixed_line_page`]: its `/BaseFont`, descriptor `/Flags`, and the words it
    /// shows, each glyph a name (`/P` reads strictly; `/C105` only through rule 1).
    struct LineFont<'a> {
        base: &'a str,
        flags: u32,
        /// More entries for the font descriptor (`/FontWeight 700`).
        descriptor: &'a str,
        /// A fill colour set before the font's words (`0.5 0 0 rg`), or none.
        paint: &'a str,
        size: f32,
        words: &'a [&'a [&'a str]],
    }

    /// A one-page PDF with one line of words in `fonts` (in order, one font after another,
    /// each word placed after the last), in `paint`, each font a CFF program whose
    /// `/Differences` from code 1 are the names it shows, without `/ToUnicode`.
    fn mixed_line_page(fonts: &[LineFont<'_>], paint: &str) -> Vec<u8> {
        const CFF: &[u8] = include_bytes!("../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");
        let mut pdf = pdfcraft_testkit::Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 300] >>");
        let resources: String = (0..fonts.len()).map(|k| format!("/F{k} {} 0 R ", 5 + 3 * k)).collect();
        pdf = pdf.obj(format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << {resources}>> >> >>"));
        let mut content = format!("BT {paint} 20 250 Td");
        for (k, f) in fonts.iter().enumerate() {
            content.push_str(&format!(" {} /F{k} {} Tf", f.paint, f.size));
            let mut code = 1;
            for w in f.words {
                let codes: String = (code..code + w.len()).map(|c| format!("{c:02x}")).collect();
                code += w.len();
                // Each word starts a word gap after the last one.
                content.push_str(&format!(" <{codes}> Tj {} 0 Td", (w.len() as f32 + 1.0) * f.size * 0.6));
            }
        }
        content.push_str(" ET");
        pdf = pdf.stream("", content);
        for (k, f) in fonts.iter().enumerate() {
            let names: Vec<&str> = f.words.iter().flat_map(|w| w.iter().copied()).collect();
            let n = names.len();
            let widths = vec!["600"; n].join(" ");
            let (descriptor, program) = (6 + 3 * k, 7 + 3 * k);
            let base = f.base;
            pdf = pdf
                .obj(format!("<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+{base} /FirstChar 1 /LastChar {n} /Widths [{widths}] /FontDescriptor {descriptor} 0 R /Encoding << /Type /Encoding /Differences [1 {}] >> >>", names.join(" ")))
                .obj(format!("<< /Type /FontDescriptor /FontName /ABCDEF+{base} /Flags {} /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 {} /FontFile3 {program} 0 R >>", f.flags, f.descriptor))
                .stream("/Subtype /Type1C", CFF);
        }
        pdf.build()
    }

    /// Item 11: one line mixing strict glyphs and glyphs rule 1 recovers, in several fonts. The
    /// two take their style from different sources (the editor's reading blocks, the font and
    /// graphics state as drawn), so numeric noise between them (a size of 11.96 against 11.955,
    /// a colour of 0.5 against 128/255) must not split a word into spans or the line into
    /// paragraphs, while every real difference (bold, italic, another family, another size)
    /// stays its own span. Strict glyphs keep Copy's style; recovered ones blend into their
    /// block. No character is ever lost.
    #[test]
    fn a_mixed_line_keeps_one_paragraph_and_its_real_style_changes() {
        let regular: &[&[&str]] = &[&["/P", "/l", "/a", "/C105", "/C110"]];
        let bold: &[&[&str]] = &[&["/C98", "/C111", "/l", "/d"]];
        let italic: &[&[&str]] = &[&["/s", "/l", "/C97", "/n", "/t"]];
        let other: &[&[&str]] = &[&["/C79", "/t", "/h", "/e", "/C114"]];
        let small: &[&[&str]] = &[&["/s", "/C109", "/a", "/l", "/l"]];
        // Bold only by the descriptor's ForceBold flag: the drawn style says bold, a name says
        // nothing.
        let heavy: &[&[&str]] = &[&["/w", "/C101", "/i", "/g", "/h", "/C116"]];
        // Bold only by a typesetter's `.B` suffix, which the drawn style reads and the editor's
        // block does not: the recovered glyphs follow their block, as Copy styles it.
        let suffixed: &[&[&str]] = &[&["/C100", "/a", "/r", "/C107"]];
        let fonts = [
            LineFont { base: "AdvTTSynthetic", flags: 32, descriptor: "", paint: "", size: 11.955, words: regular },
            LineFont { base: "AdvTTSynthetic-Bold", flags: 32, descriptor: "", paint: "", size: 11.955, words: bold },
            LineFont { base: "AdvTTSynthetic-Italic", flags: 32, descriptor: "", paint: "", size: 11.955, words: italic },
            LineFont { base: "OtherSans", flags: 32, descriptor: "", paint: "", size: 11.955, words: other },
            LineFont { base: "Plainname", flags: 32 | 262_144, descriptor: "", paint: "", size: 11.955, words: heavy },
            LineFont { base: "Typeset.B", flags: 32, descriptor: "", paint: "", size: 11.955, words: suffixed },
            LineFont { base: "AdvTTSynthetic", flags: 32, descriptor: "", paint: "", size: 9.0, words: small },
        ];
        let mut s = Session::new();
        let id = s.open("mixed.pdf", None, Arc::new(mixed_line_page(&fonts, "0.5 0 0 rg")), None).unwrap();
        let doc = s.get(id).unwrap();
        let layer = recovered_layer(&s, id).expect("recovered layer");
        let all: Vec<usize> = (0..layer.text.glyphs.len()).collect();
        let copy = doc.rich_copy_recovered(0, &layer, &all).unwrap();
        assert_eq!(copy.plain_text(), "Plain bold slant Other weight dark small", "every character comes through");
        assert_eq!(copy.text.paragraphs.len(), 1, "one line, one paragraph: {:#?}", copy.text.paragraphs);
        let spans: Vec<(&str, &ClipStyle)> = copy.text.paragraphs[0].iter().map(|s| (s.text.trim_end(), &s.style)).collect();
        let words: Vec<&str> = spans.iter().map(|s| s.0).collect();
        assert_eq!(words, ["Plain", "bold", "slant", "Other", "weight", "dark", "small"], "one span per word: {spans:#?}");
        let style = |w: &str| spans.iter().find(|s| s.0 == w).map(|s| s.1).unwrap();
        assert!(!style("Plain").bold && !style("Plain").italic);
        assert!(style("bold").bold && !style("bold").italic, "{:?}", style("bold"));
        assert!(style("slant").italic && !style("slant").bold, "{:?}", style("slant"));
        assert_ne!(style("Other").families, style("Plain").families);
        assert!(style("weight").bold, "{:?}", style("weight"));
        assert!(!style("dark").bold, "the recovered glyphs follow their block, as Copy styles it: {:?}", style("dark"));
        assert!((style("small").size - 9.0).abs() < 0.05 && (style("Plain").size - 11.955).abs() < 0.05, "{spans:#?}");
        assert!(copy.html.contains("<strong>bold") && copy.html.contains("<em>slant"), "{}", copy.html);
    }

    /// Review finding: a word in a CMYK colour mixing strict and recovered glyphs stays one span.
    /// The reading block reads `0 0 0 1 k` as black, the drawn glyph through the colour space as
    /// a dark grey; a recovered glyph in its block's font and size takes the block's style,
    /// colour included, so the colour never splits the run.
    #[test]
    fn a_cmyk_word_mixing_strict_and_recovered_glyphs_is_one_span() {
        let word: &[&[&str]] = &[&["/P", "/l", "/a", "/C105", "/C110"]];
        let dark: &[&[&str]] = &[&["/C100", "/a", "/r", "/C107"]];
        let fonts = [
            LineFont { base: "AdvTTSynthetic", flags: 32, descriptor: "", paint: "", size: 12.0, words: word },
            LineFont { base: "Typeset.B", flags: 32, descriptor: "", paint: "", size: 12.0, words: dark },
        ];
        for paint in ["0 0 0 1 k", "0.2 0.6 0.1 0.05 k"] {
            let mut s = Session::new();
            let id = s.open("cmyk.pdf", None, Arc::new(mixed_line_page(&fonts, paint)), None).unwrap();
            let doc = s.get(id).unwrap();
            let layer = recovered_layer(&s, id).expect("recovered layer");
            let all: Vec<usize> = (0..layer.text.glyphs.len()).collect();
            let copy = doc.rich_copy_recovered(0, &layer, &all).unwrap();
            assert_eq!(copy.plain_text(), "Plain dark", "{paint}");
            let spans: Vec<(&str, &ClipStyle)> = copy.text.paragraphs[0].iter().map(|s| (s.text.trim_end(), &s.style)).collect();
            assert_eq!(spans.iter().map(|s| s.0).collect::<Vec<_>>(), ["Plain", "dark"], "{paint}: {spans:#?}");
            // The recovered glyphs follow their block, as Copy styles it.
            assert!(!spans[1].1.bold, "{paint}: {spans:#?}");
        }
    }

    /// Review finding: a strict glyph's style doesn't depend on what else is selected. The
    /// strict glyphs of a word in a typesetter's `.B` font copy in their block's style, as Copy
    /// gives it, whether or not the word's recovered glyphs are selected too.
    #[test]
    fn a_strict_glyphs_style_does_not_depend_on_the_selection() {
        let dark: &[&[&str]] = &[&["/C100", "/a", "/r", "/C107"]];
        let fonts = [LineFont { base: "Typeset.B", flags: 32, descriptor: "", paint: "", size: 12.0, words: dark }];
        let mut s = Session::new();
        let id = s.open("dark.pdf", None, Arc::new(mixed_line_page(&fonts, "0 g")), None).unwrap();
        let doc = s.get(id).unwrap();
        let layer = recovered_layer(&s, id).expect("recovered layer");
        let strict: Vec<usize> = (0..layer.text.glyphs.len()).filter(|&i| layer.recovery_of(i).is_none()).collect();
        let all: Vec<usize> = (0..layer.text.glyphs.len()).collect();
        let alone = doc.rich_copy_recovered(0, &layer, &strict).unwrap();
        let together = doc.rich_copy_recovered(0, &layer, &all).unwrap();
        assert_eq!(alone.plain_text(), "ar");
        let style = |c: &super::RichCopy| c.text.paragraphs[0][0].style.clone();
        // Copy's judgement: the reading block reads no bold in `Typeset.B`, so neither copy does.
        assert!(!style(&alone).bold, "{:?}", style(&alone));
        assert_eq!(style(&alone), style(&together));
    }

    /// Review finding: a glyph past the copy's caps isn't copied, so running out of match steps
    /// on it leaves the styles complete: only glyphs that are copied count. (A selection with
    /// nothing recovered is Copy's copy, which counts as Copy always has.)
    #[test]
    fn glyphs_past_the_cap_do_not_leave_the_styles_incomplete() {
        let mut s = Session::new();
        let id = s.open("advtt.pdf", None, Arc::new(decimal_name_page(DECIMAL_NAMES, true)), None).unwrap();
        let doc = s.get(id).unwrap();
        let real = recovered_layer(&s, id).expect("recovered layer");
        let mut text = (*real.text).clone();
        // The first glyph fills the copy's text cap; nothing after it is copied.
        text.glyphs[0].text = "a".repeat(pdfcraft_export::MAX_CLIP_TEXT_BYTES);
        let n = text.glyphs.len();
        let strict: Vec<Option<usize>> = (0..n).map(|i| real.strict_index_of(i)).collect();
        let strict_len = strict.iter().flatten().count();
        let layer = pdfcraft_render::RecoveredText::new(Arc::new(text), real.recovery.clone(), strict, real.style.clone(), strict_len);
        let rec = (0..n).rev().find(|&i| layer.recovery_of(i).is_some()).expect("a recovered glyph");
        // The first glyph, then a recovered one past the cap.
        let steps = (0..10_000)
            .find(|&k| !doc.rich_copy_formatted_within(0, &layer.text, &[0, rec], k, Some(&layer)).unwrap().styles_incomplete)
            .expect("a budget");
        let all: Vec<usize> = std::iter::once(0).chain((1..n).filter(|&i| i != rec)).chain([rec]).collect();
        let copy = doc.rich_copy_formatted_within(0, &layer.text, &all, steps, Some(&layer)).unwrap();
        assert!(copy.truncated);
        assert!(!copy.styles_incomplete, "the glyphs past the cap were never copied");
    }

    /// Review finding: a font's bold and italic don't depend on what else the page holds. A
    /// font bold only by its descriptor's `/FontWeight` copies bold, strict glyphs and recovered
    /// ones alike, on a page with both and on pages with only one of them.
    #[test]
    fn a_fonts_weight_is_the_same_on_every_page() {
        let strict: &[&[&str]] = &[&["/P", "/l", "/a", "/n"]];
        let recovered: &[&[&str]] = &[&["/C105", "/C110", "/C107"]];
        let font = |words, size| LineFont { base: "Plainname", flags: 32, descriptor: "/FontWeight 700", paint: "", size, words };
        let copy = |fonts: &[LineFont<'_>]| {
            let mut s = Session::new();
            let id = s.open("weight.pdf", None, Arc::new(mixed_line_page(fonts, "0 g")), None).unwrap();
            let doc = s.get(id).unwrap();
            let c = match recovered_layer(&s, id) {
                Some(layer) => doc.rich_copy_recovered(0, &layer, &(0..layer.text.glyphs.len()).collect::<Vec<_>>()).unwrap(),
                None => {
                    let text = page_text(&s, id);
                    doc.rich_copy_formatted(0, &text, &(0..text.glyphs.len()).collect::<Vec<_>>(), None).unwrap()
                }
            };
            c.text.paragraphs.iter().flatten().map(|s| (s.text.trim().to_owned(), s.style.bold)).collect::<Vec<_>>()
        };
        let both = copy(&[font(strict, 12.0), font(recovered, 9.0)]);
        assert_eq!(both, [("Plan".to_owned(), true), ("ink".to_owned(), true)], "{both:?}");
        assert_eq!(copy(&[font(strict, 12.0)]), [("Plan".to_owned(), true)]);
        assert_eq!(copy(&[font(recovered, 9.0)]), [("ink".to_owned(), true)]);
    }

    /// Review finding: a word of recovered glyphs that lies past its font's reading block on the
    /// same line, in the same font and size, runs on to the block and takes its style: a
    /// CMYK-black word copies black, as the strict word before it does, not as the drawn grey.
    #[test]
    fn a_recovered_word_beside_its_block_takes_the_blocks_style() {
        // The recovered word a gap further on, in its own font resource (the same font), which
        // the editor reads no block for.
        let strict: &[&[&str]] = &[&["/P", "/l", "/a", "/n"]];
        let recovered: &[&[&str]] = &[&["/C105", "/C110", "/C107"]];
        let fonts = [
            LineFont { base: "AdvTTSynthetic", flags: 32, descriptor: "", paint: "", size: 12.0, words: strict },
            LineFont { base: "AdvTTSynthetic", flags: 32, descriptor: "", paint: "30 0 Td", size: 12.0, words: recovered },
        ];
        let mut s = Session::new();
        let id = s.open("cmyk.pdf", None, Arc::new(mixed_line_page(&fonts, "0 0 0 1 k")), None).unwrap();
        let doc = s.get(id).unwrap();
        let layer = recovered_layer(&s, id).expect("recovered layer");
        let all: Vec<usize> = (0..layer.text.glyphs.len()).collect();
        let copy = doc.rich_copy_recovered(0, &layer, &all).unwrap();
        let spans: Vec<_> = copy.text.paragraphs.iter().flatten().collect();
        let words: Vec<&str> = spans.iter().flat_map(|s| s.text.split_whitespace()).collect();
        assert_eq!(words, ["Plan", "ink"], "{spans:#?}");
        // The two words share one style (one span, or two alike): the block's, black.
        assert!(spans.iter().all(|s| s.style == spans[0].style), "the recovered word takes its block's style: {spans:#?}");
        assert_eq!(spans[0].style.color, [0.0, 0.0, 0.0]);
    }

    /// Copy with formatting is Copy's copy plus the recovered glyphs: a selection with nothing
    /// recovered copies byte for byte as Copy does, and beside recovered glyphs two strict words
    /// whose colours differ by less than 1/255 stay two spans, as Copy keeps them (noise is
    /// forgiven only between a recovered glyph styled as drawn and the span before it).
    #[test]
    fn copy_with_formatting_keeps_copys_strict_styles() {
        let a: &[&[&str]] = &[&["/P", "/l", "/a", "/n"]];
        let b: &[&[&str]] = &[&["/l", "/a", "/n", "/d"]];
        let r: &[&[&str]] = &[&["/C105", "/C110", "/C107"]];
        let fonts = [
            LineFont { base: "AdvTTSynthetic", flags: 32, descriptor: "", paint: "0.5 0 0 rg", size: 12.0, words: a },
            LineFont { base: "AdvTTSynthetic", flags: 32, descriptor: "", paint: "0.502 0 0 rg", size: 12.0, words: b },
            LineFont { base: "Othername", flags: 32, descriptor: "", paint: "", size: 9.0, words: r },
        ];
        let mut s = Session::new();
        let id = s.open("noise.pdf", None, Arc::new(mixed_line_page(&fonts, "0 g")), None).unwrap();
        let doc = s.get(id).unwrap();
        let strict = page_text(&s, id);
        let all: Vec<usize> = (0..strict.glyphs.len()).collect();
        let copy = doc.rich_copy(0, &strict, &all).unwrap();
        let formatted = doc.rich_copy_formatted(0, &strict, &all, None).unwrap();
        assert_eq!((&formatted.html, formatted.plain_text()), (&copy.html, copy.plain_text()), "nothing recovered: Copy's copy");
        let layer = recovered_layer(&s, id).expect("recovered layer");
        let with = doc.rich_copy_recovered(0, &layer, &(0..layer.text.glyphs.len()).collect::<Vec<_>>()).unwrap();
        let spans: Vec<(String, f64)> = with.text.paragraphs.iter().flatten().map(|s| (s.text.trim().to_owned(), s.style.size)).collect();
        let main: Vec<(String, f64)> = copy.text.paragraphs.iter().flatten().map(|s| (s.text.trim().to_owned(), s.style.size)).collect();
        assert_eq!(spans.get(..main.len()), Some(&main[..]), "the strict words keep Copy's spans: {spans:?}");
        assert_eq!(main.len(), 2, "{main:?}");
        assert_eq!(spans.last().map(|s| s.0.as_str()), Some("ink"));
    }

    /// Review finding: a recovered word whose block beside it was never tested, because the match
    /// steps ran out, marks the styles incomplete: whatever the budget, the word either takes its
    /// block's style or the copy says some styles are missing.
    #[test]
    fn running_out_of_steps_beside_a_block_leaves_the_styles_incomplete() {
        let strict: &[&[&str]] = &[&["/P", "/l", "/a", "/n"]];
        let recovered: &[&[&str]] = &[&["/C105", "/C110", "/C107"]];
        let fonts = [
            LineFont { base: "AdvTTSynthetic", flags: 32, descriptor: "", paint: "", size: 12.0, words: strict },
            LineFont { base: "AdvTTSynthetic", flags: 32, descriptor: "", paint: "30 0 Td 0 0 0 1 k", size: 12.0, words: recovered },
        ];
        let mut s = Session::new();
        let id = s.open("beside.pdf", None, Arc::new(mixed_line_page(&fonts, "0 0 0 1 k")), None).unwrap();
        let doc = s.get(id).unwrap();
        let layer = recovered_layer(&s, id).expect("recovered layer");
        let all: Vec<usize> = (0..layer.text.glyphs.len()).collect();
        let mut short = 0;
        for steps in 0..400 {
            let copy = doc.rich_copy_formatted_within(0, &layer.text, &all, steps, Some(&layer)).unwrap();
            let spans: Vec<_> = copy.text.paragraphs.iter().flatten().collect();
            let words: Vec<&str> = spans.iter().flat_map(|s| s.text.split_whitespace()).collect();
            let blended = words == ["Plan", "ink"] && spans.iter().all(|s| s.style == spans[0].style);
            if !blended {
                short += 1;
                assert!(copy.styles_incomplete, "{steps} steps: {spans:#?}");
            }
        }
        assert!(short > 0, "some budget is too short");
    }

    /// A font the rules reject leaves the page without a recovered layer.
    #[test]
    fn fonts_the_rules_reject_have_no_recovered_layer() {
        for odd in ["/C3", "/G5F", "/g3", "/C1111"] {
            let mut s = Session::new();
            let id = s.open("advtt.pdf", None, Arc::new(decimal_name_page(&format!("/C83 /C111 {odd}"), true)), None).unwrap();
            assert!(recovered_layer(&s, id).is_none(), "{odd}");
        }
    }
}
