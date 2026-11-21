//! Edit a PDF ▸ Edit text & images: boxes around the paragraphs and images already on the page.
//! Click a paragraph to edit it in place (⌘Enter or clicking away applies and rewraps it to the
//! box, Esc cancels); drag it to move it, or drag the handle on its left or right edge to rewrap
//! it to a new width. Click an image to select it: drag to move, drag a corner to resize (keeping
//! its proportions), right-click for rotate, flip, replace, save and delete; Delete removes it.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Stroke, pos2};
use pdfcraft_engine::{Edit, ObjectKind, ObjectTarget};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::page_cursor::Layer;
use crate::page_keys::PageKeys;
use crate::page_pointer::PagePointer;
use crate::page_router::{Hit, Owner, PagePress, ParagraphPart};

mod background;
mod image;
mod layout;

pub(crate) use background::TextBackground;
pub use image::{CropMode, DragTarget, ImageAction, ImageDrag, ImageHandle, ImageSelection};
pub(crate) use image::{
    ImageCx, Matrix, body_hit as picture_hit, crop_hit, handle_hit as picture_handle_hit, image_context_menu, image_input, picture_context,
    picture_keys, quad_contains, user_to_screen,
};
#[cfg(test)]
use image::{rotate_knob, turn_degrees, turn_degrees_delta};
#[cfg(test)]
use layout::{DARK_EDITOR_FILL, contrast};
pub(crate) use layout::{caret_color_on, editor_fill, editor_font, install_source_fonts};
use layout::{color32, editor_galley, editor_job, look_of, luminance, source_look, unseen};

const ACCENT: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

#[derive(Clone, Copy, PartialEq)]
enum TextTarget {
    Block(usize),
    Added(usize),
}

/// How many faint Edit-text outlines are drawn per page per frame. CAD plots can draw
/// ~1000 tiny images on one page; stroking all of them every frame (plus a hover query
/// per box) stalls the UI. Past this, only the selected and hovered boxes are outlined:
/// hit-testing still covers every image, so nothing becomes unselectable.
pub(crate) const MAX_OUTLINES: usize = 250;

/// Format text ▸ Superscript or Subscript on `text`: on the `selection` (bytes of `text`), or all
/// of it without one ([`pdfcraft_engine::text_script::toggle`]). A selection follows the
/// converted characters. Whether the text changed.
pub(crate) fn toggle_script(text: &mut String, selection: &mut Option<std::ops::Range<usize>>, script: pdfcraft_engine::text_script::Script) -> bool {
    if selection.as_ref().is_some_and(|r| r.is_empty()) {
        *selection = None;
    }
    let Some((toggled, range)) = pdfcraft_engine::text_script::toggle(text, selection.as_ref(), script) else { return false };
    *text = toggled;
    if selection.is_some() {
        *selection = Some(range);
    }
    true
}

/// A paragraph being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct LineEditor {
    pub selection: Option<std::ops::Range<usize>>,
    pub font_resource: Option<String>,
    pub page: usize,
    pub block: usize,
    pub text: String,
    original: String,
    rect: Rect,
    /// The original PDF rectangle, in user space. It is converted again each frame so the editor
    /// stays attached while the page is zoomed, scrolled, or rotated.
    source_rect: [f32; 4],
    multiline: bool,
    source: Option<(pdfcraft_engine::TextBlock, Vec<pdfcraft_engine::TextLine>)>,
    pub(crate) fonts: BTreeMap<String, String>,
    /// The draft extends outside the source container; its frame follows the live draft bounds.
    pub overflow: bool,
    /// The initial source container width in screen pixels.
    max_width: f32,
    /// Current screen-space size. Recomputed from the current page transform before painting.
    size: f32,
    /// Take the keyboard focus on the next frame.
    pub focus: bool,
    /// Whether the text box had the keyboard at the end of the editor's last frame
    /// ([`lost_focus_once`]).
    had_focus: bool,
    /// The Format text panel's values, and what the paragraph had (to send only changes).
    pub look: pdfcraft_engine::AddedText,
    look0: pdfcraft_engine::AddedText,
    /// A standard family was picked from the font list. The family guessed from a PDF font name
    /// (Calibri reads as Helvetica) can be the one picked, so comparing `look` with `look0` alone
    /// would miss the choice.
    family_chosen: bool,
    size_chosen: bool,
    /// Underline, line spacing (× size; 0 = the paragraph's own), character spacing (pt) and
    /// horizontal scale (%), and what they were.
    pub extras: Extras,
    extras0: Extras,
    /// The text the box's own Undo history starts from; see [`forget_typing_before`].
    typing_from: Option<String>,
}

/// The paragraph editor's text box: one for the tabs of a pane (they share it), and one for each pane
/// of split view, which can show an editor in each.
pub(crate) fn input_id(place: crate::panes::Place) -> egui::Id {
    egui::Id::unique(("edit-text-line-input", place))
}

/// The page text box `id` keeps its own Undo history, across every paragraph or box it has shown
/// in every tab (they share the id). Once the page has `text` (the box opened on it, or formatting
/// applied it), that history is cleared: Undo in the box takes back only what was typed since, and
/// then reaches the document. Kept, it put back text from another paragraph, or the whole paragraph
/// as it was before a format change, into a box showing the page as it is now. `from` is the box's
/// own record of that; which document `doc` last used the history is kept beside it, since a box
/// in another tab on the same text would otherwise take that document's typing with its Redo.
/// When another document's box takes the text box, the box it leaves keeps its history (and
/// caret) aside, and gets it back on its return, so its typing can still be undone there. Each
/// box claims the text box when it is drawn, and Undo and Redo from the menu claim it first too,
/// since they can run before the box is drawn again after a tab switch.
pub(crate) fn forget_typing_before(ctx: &egui::Context, id: egui::Id, doc: pdfcraft_engine::DocId, from: &mut Option<String>, text: &str) {
    let owner = id.with("typing-doc");
    let last = ctx.data(|d| d.get_temp::<pdfcraft_engine::DocId>(owner));
    if from.as_deref() == Some(text) && last == Some(doc) {
        return;
    }
    let aside = |doc: pdfcraft_engine::DocId| id.with(("typing-aside", doc));
    let aligns = id.with(ALIGN_HISTORY);
    if last.is_some_and(|l| l != doc) {
        // The narrowed-word anchor belongs to the document that made it.
        ctx.data_mut(|d| d.remove::<[usize; 4]>(id.with("narrowed word")));
    }
    if let Some(left) = last.filter(|l| *l != doc)
        && let Some(state) = egui::TextEdit::load_state(ctx, id)
    {
        // A copy of its own: a state's history is shared between its clones, and the next box
        // clears it in place. The rows' alignments by text go with it.
        let mut kept = egui::text_edit::TextEditState::default();
        kept.cursor = state.cursor;
        kept.set_undoer(state.undoer());
        ctx.data_mut(|d| {
            let rows = d.get_temp::<AlignHistory>(aligns).unwrap_or_default();
            d.insert_temp(aside(left), (kept, rows));
        });
    }
    // Only the same box, still on the text it opened on, takes its history back.
    let back =
        ctx.data_mut(|d| d.remove_temp::<(egui::text_edit::TextEditState, AlignHistory)>(aside(doc))).filter(|_| from.as_deref() == Some(text));
    // A box new to the text box starts with nothing of the last one's: no history, caret,
    // selection or rows' alignments by text.
    let (state, rows) = back.unwrap_or_default();
    egui::TextEdit::store_state(ctx, id, state);
    ctx.data_mut(|d| {
        d.insert_temp(owner, doc);
        d.insert_temp(aligns, rows);
    });
    *from = Some(text.to_owned());
}

/// Each text a page box has shown through its own typing, with its rows' alignments then, oldest
/// last: egui's history keeps only the text and the caret.
type AlignHistory = Vec<(String, Vec<Vec<Option<pdfcraft_engine::TextAlign>>>)>;
const ALIGN_HISTORY: &str = "aligns-by-text";
/// As many texts as egui's own history keeps.
const ALIGN_HISTORY_LEN: usize = 100;

/// The page text box `id` went from `before` to `after` through its own typing (typing, Undo or
/// Redo), and each list of row alignments in `aligns` follows. A text the box showed before gets
/// back the alignments it had then, so Undo bringing back a deleted row brings back its
/// alignment; a new text follows the edit ([`pdfcraft_engine::paragraph_align::remap`]). An
/// alignment picked since makes the older texts' alignments stale, and they are dropped.
pub(crate) fn follow_typing(
    ctx: &egui::Context,
    id: egui::Id,
    before: &str,
    after: &str,
    aligns: &mut [&mut Vec<Option<pdfcraft_engine::TextAlign>>],
) {
    let now: Vec<Vec<_>> = aligns.iter().map(|a| a.to_vec()).collect();
    // Where the edit left the caret, as a byte of `after`: it says which of two same rows changed.
    let caret = egui::TextEdit::load_state(ctx, id)
        .and_then(|s| s.cursor.char_range())
        .map(|r| after.char_indices().nth(r.primary.index.into()).map_or(after.len(), |(at, _)| at));
    let next = ctx.data_mut(|d| {
        let seen = d.get_temp_mut_or_default::<AlignHistory>(id.with(ALIGN_HISTORY));
        if seen.iter().any(|(text, was)| text == before && *was != now) {
            seen.clear();
        }
        remember(seen, before, now.clone());
        let next = match seen.iter().rev().find(|(text, _)| text == after) {
            Some((_, was)) if was.len() == now.len() => was.clone(),
            _ => now.iter().map(|a| pdfcraft_engine::paragraph_align::remap_at(before, after, a, caret)).collect(),
        };
        remember(seen, after, next.clone());
        next
    });
    for (slot, a) in aligns.iter_mut().zip(next) {
        **slot = a;
    }
}

fn remember(seen: &mut AlignHistory, text: &str, aligns: Vec<Vec<Option<pdfcraft_engine::TextAlign>>>) {
    seen.retain(|(t, _)| t != text);
    seen.push((text.to_owned(), aligns));
    if seen.len() > ALIGN_HISTORY_LEN {
        seen.remove(0);
    }
}

/// Select `range` (bytes of `text`) in the page text box `id` again. egui keeps a box's selection
/// only while it has the focus, so a click on Format text drops it; given back as formatting
/// returns the focus, it stays shown, and a second choice reaches the same text.
pub(crate) fn reselect(ctx: &egui::Context, id: egui::Id, text: &str, range: &std::ops::Range<usize>) {
    let chars = |at: usize| text.get(..at).map(|s| s.chars().count());
    let (Some(start), Some(end)) = (chars(range.start), chars(range.end)) else { return };
    if start < end {
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(start), egui::text::CCursor::new(end))));
        egui::TextEdit::store_state(ctx, id, state);
    }
}

/// The word a double-click at `caret` (a char index of `text`) means: the word whose letter the
/// caret sits before, or after it where the caret ends a word. egui's own double-click reaches back
/// to the previous word and on to the next one wherever the caret sits at a word edge, so
/// double-clicking the left half of a one-letter "a" in "Take a calm" selected "Take a".
fn word_at_caret(text: &str, caret: usize) -> Option<std::ops::Range<usize>> {
    use egui::text::CCursor;
    use egui::text_selection::text_cursor_state::{ccursor_next_word, ccursor_previous_word};
    let solid = |at: usize| text.chars().nth(at).is_some_and(|c| !c.is_whitespace());
    let (start, end) = if solid(caret) {
        (ccursor_previous_word(text, CCursor::new(caret + 1)), ccursor_next_word(text, CCursor::new(caret)))
    } else if caret > 0 && solid(caret - 1) {
        (ccursor_previous_word(text, CCursor::new(caret)), ccursor_next_word(text, CCursor::new(caret - 1)))
    } else {
        return None;
    };
    Some(usize::from(start.index)..usize::from(end.index))
}

/// The char index of the character the pointer (an offset in galley space) is over, when it is over a
/// glyph. `cursor_from_pos` reduces any hit to the nearer caret, so the left half of the space
/// after "Take" and the right half of the "e" both come back as caret 4.
fn char_under(galley: &egui::Galley, pos: egui::Vec2) -> Option<usize> {
    use egui::text::CCursor;
    let caret = usize::from(galley.cursor_from_pos(pos).index);
    // A glyph starts on the row that begins with it and ends on the row it is on, so at a soft wrap
    // the start is asked for with `prefer_next_row` and the end without.
    let start = |i: usize| galley.pos_from_cursor(CCursor { index: i.into(), prefer_next_row: true });
    let end = |i: usize| galley.pos_from_cursor(CCursor::new(i));
    [caret.checked_sub(1), Some(caret)].into_iter().flatten().find(|&i| {
        let (from, to) = (start(i), end(i + 1));
        // The pointer must be on the glyph's own row: a click in the blank space after a short
        // row hits the caret that starts the next row, whose first glyph shares the x range.
        (from.center().y - to.center().y).abs() < 1.0 && from.y_range().contains(pos.y) && from.min.x <= pos.x && pos.x < to.min.x
    })
}

/// Narrow the selection a double-click just made in the text box `id` to the word under the
/// pointer (see [`word_at_caret`]), and keep a following Shift-click extending from that word.
/// Shown from the next pass on. Runs once per pass after the text box.
pub(crate) fn narrow_double_click(ui: &egui::Ui, id: egui::Id, text: &str, output: &egui::text_edit::TextEditOutput) {
    use egui::text::{CCursor, CCursorRange};
    let marker = id.with("narrowed word");
    // The word, and the selection as this function left it: another one (a key moved it, text was
    // typed, the box was reopened) ends the anchor.
    let kept: Option<[usize; 4]> = ui.ctx().data(|d| d.get_temp(marker));
    let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    let sorted = |r: CCursorRange| r.sorted_cursors().map(|c| usize::from(c.index));
    let ends = |r: CCursorRange| (usize::from(r.primary.index), usize::from(r.secondary.index));
    let mut selected = state.cursor.char_range();
    let mut changed = false;
    // A press and its release can arrive in one frame (no button down at the end of it); egui still
    // handles that press, so it counts here too.
    let down = output.response.is_pointer_button_down_on() || (output.response.hovered() && ui.input(|i| i.pointer.any_pressed()));
    let stored = kept.filter(|m| down || selected.map(ends) == Some((m[2], m[3])));
    let narrowed = stored.map(|m| [m[0], m[1]]);
    let mut keep = narrowed;
    let shift = ui.input(|i| i.modifiers.shift);
    // egui keeps the word a double-click selected as the anchor of the next Shift-clicks, so each
    // click extends from that word, in either direction, and never drops it. It cannot be set from
    // outside and `set_char_range` clears it, so a selection narrowed here has none: egui's plain
    // Shift-click would keep only the end the selection began at, which after the first extension
    // is the word's far end. Give every click of the sequence the result egui gives: the pointer
    // end is where egui put the primary cursor, the other end is the far end of the word.
    if let (Some([start, end]), Some(range), true) = (narrowed, selected, down) {
        if shift {
            let pointer = usize::from(range.primary.index);
            let fixed = if pointer < start {
                CCursorRange { primary: range.primary, secondary: CCursor::new(end), h_pos: None }
            } else if pointer > end {
                CCursorRange { primary: range.primary, secondary: CCursor::new(start), h_pos: None }
            } else {
                CCursorRange::two(CCursor::new(start), CCursor::new(end))
            };
            if ends(fixed) != ends(range) {
                state.cursor.set_char_range(Some(fixed));
                selected = Some(fixed);
                changed = true;
            }
        } else {
            // A plain press starts a new selection.
            keep = None;
        }
    }
    // With Shift held egui extends the selection instead; keep that range (egui's own check in
    // `pointer_interaction` is `double_clicked() && !shift`).
    let double_click = output.response.double_clicked() && !shift;
    // Only over a glyph: a double-click on the space after "Take" is left to egui, though the
    // caret it hits ends the word before it.
    let target = double_click
        .then(|| output.response.interact_pointer_pos())
        .flatten()
        .and_then(|pointer| char_under(&output.galley, pointer - output.galley_pos))
        .filter(|&at| text.chars().nth(at).is_some_and(|c| !c.is_whitespace()))
        .and_then(|at| word_at_caret(text, at));
    if let Some(word) = target {
        if selected.map(sorted) != Some([word.start, word.end]) {
            let range = CCursorRange::two(CCursor::new(word.start), CCursor::new(word.end));
            state.cursor.set_char_range(Some(range));
            selected = Some(range);
            changed = true;
        }
        keep = Some([word.start, word.end]);
    }
    // The anchor lasts while the selection still holds the word; typing or a new selection ends it.
    let keep = keep.filter(|&[start, end]| selected.map(sorted).is_some_and(|[low, high]| low <= start && end <= high));
    let now = keep.zip(selected).map(|([start, end], range)| [start, end, ends(range).0, ends(range).1]);
    if now != kept {
        ui.ctx().data_mut(|d| match now {
            Some(m) => {
                d.insert_temp(marker, m);
            }
            None => {
                d.remove::<[usize; 4]>(marker);
            }
        });
    }
    if changed {
        egui::TextEdit::store_state(ui.ctx(), id, state);
        ui.ctx().request_repaint();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extras {
    pub underline: bool,
    pub line_spacing: f64,
    /// Space between paragraphs in points (a line break in the box starts one); 0 = unset (a
    /// new paragraph gets half the size; rows the page had keep their space).
    pub paragraph_spacing: f64,
    pub char_spacing: f64,
    pub scale: f64,
}

impl Default for Extras {
    fn default() -> Self {
        Extras { underline: false, line_spacing: 0.0, paragraph_spacing: 0.0, char_spacing: 0.0, scale: 100.0 }
    }
}

impl LineEditor {
    /// What Format text's font list shows: the chosen page font, a standard family picked
    /// instead, or the paragraph's own font (its `/BaseFont` without the subset tag).
    pub(crate) fn font_shown(&self, sources: &[(String, String)]) -> String {
        if let Some(face) = self.font_resource.as_ref().and_then(|r| sources.iter().find(|(res, _)| res == r)).map(|(_, face)| face) {
            return face.clone();
        }
        if self.look.family != self.look0.family || self.family_chosen {
            return self.look.family.label().to_string();
        }
        let base = self.source.as_ref().map(|(block, _)| block.base_font.as_str()).unwrap_or_default();
        // "ABCDEF+Calibri" is a subset of Calibri.
        let name = pdfcraft_engine::strip_subset_tag(base);
        if name.is_empty() { tl!("Keep source fonts").to_string() } else { name.to_string() }
    }

    /// Back to the paragraph's own font: no page font chosen and the family it started with.
    pub(crate) fn keep_source_font(&mut self) {
        self.font_resource = None;
        self.look.family = self.look0.family;
        self.family_chosen = false;
    }

    /// A standard family picked from the font list, replacing the paragraph's font even when it
    /// is the family the paragraph's font name reads as.
    pub(crate) fn choose_family(&mut self, family: pdfcraft_engine::FontFamily) {
        self.font_resource = None;
        self.look.family = family;
        self.family_chosen = true;
    }

    pub(crate) fn has_changes(&self) -> bool {
        self.changes_with(&self.text)
    }

    /// Whether the paragraph with `text` in it differs from what was opened.
    fn changes_with(&self, text: &str) -> bool {
        text != self.original || self.look != self.look0 || self.extras != self.extras0 || self.family_chosen || self.size_chosen
    }

    /// Extra width permitted by the editor. Source containers now remain fixed.
    pub fn growth(&self) -> Option<f32> {
        (self.max_width > self.rect.width()).then_some(self.max_width)
    }

    /// Whether the edited text differs from the original (unsaved changes).
    pub fn has_unsaved_text(&self) -> bool {
        self.text != self.original
    }

    /// The formatting the panel changed.
    pub fn style(&self) -> pdfcraft_engine::BlockStyle {
        let (l, o) = (&self.look, &self.look0);
        let mut style = pdfcraft_engine::BlockStyle {
            resource: self.font_resource.clone(),
            family: (l.family != o.family || self.family_chosen || l.bold != o.bold || l.italic != o.italic).then_some((l.family, l.bold, l.italic)),
            size: (self.size_chosen || l.size != o.size).then_some(l.size),
            color: (l.color != o.color).then_some(l.color),
            // The whole paragraph's alignment when it was picked, or when the rows' own were
            // dropped for it (the rows are where those put them, so leaving it out keeps them there).
            align: (l.align != o.align || l.aligns.is_empty() && !o.aligns.is_empty()).then_some(l.align),
            underline: (self.extras.underline != self.extras0.underline).then_some(self.extras.underline),
            line_spacing: (self.extras.line_spacing != self.extras0.line_spacing && self.extras.line_spacing > 0.0)
                .then_some(self.extras.line_spacing),
            paragraph_spacing: (self.extras.paragraph_spacing != self.extras0.paragraph_spacing).then_some(self.extras.paragraph_spacing),
            char_spacing: (self.extras.char_spacing != self.extras0.char_spacing).then_some(self.extras.char_spacing),
            scale: (self.extras.scale != self.extras0.scale).then_some(self.extras.scale),
            ..Default::default()
        };
        // Rows aligned on their own, sent with every rewrite of the paragraph while it is open
        // (a colour or size change too): nothing in the page records them, so a rewrite without
        // them would undo them. Alone, they are sent only when they changed.
        if l.aligns != o.aligns || self.text != self.original || style != pdfcraft_engine::BlockStyle::default() {
            style.aligns = l.aligns.clone();
        }
        if let Some(range) = &self.selection
            && (style.family.is_some()
                || style.resource.is_some()
                || style.size.is_some()
                || style.color.is_some()
                || style.underline.is_some()
                || style.char_spacing.is_some()
                || style.scale.is_some())
        {
            style.ranges.push(pdfcraft_engine::RangeStyle {
                range: range.clone(),
                family: style.family.take(),
                resource: style.resource.take(),
                size: style.size.take(),
                color: style.color.take(),
                underline: style.underline.take(),
                char_spacing: style.char_spacing.take(),
                scale: style.scale.take(),
            });
        }
        style
    }

    /// An explicit size commit also normalizes a mixed selection to the displayed value.
    pub(crate) fn choose_size(&mut self, size: f64) {
        self.look.size = size;
        self.size_chosen = true;
    }

    /// After applying formatting: the paragraph now has it. Its old byte selection is cleared;
    /// the rewritten paragraph may have different text or geometry.
    pub fn applied(&mut self) {
        self.size_chosen = false;
        self.font_resource = None;
        self.family_chosen = false;
        self.look0 = self.look.clone();
        self.extras0 = self.extras;
        self.original = self.text.clone();
        self.selection = None;
        // The page now has this text: the box's history restarts, even when the format left the
        // text as it was, so a Redo from before can't put undone typing back on top of it.
        self.typing_from = None;
        self.focus = true;
    }

    /// Take the shared text box for this paragraph ([`forget_typing_before`]): its own history,
    /// or none once the page has its text.
    pub(crate) fn claim_typing(&mut self, ctx: &egui::Context, doc: pdfcraft_engine::DocId, place: crate::panes::Place) {
        forget_typing_before(ctx, input_id(place), doc, &mut self.typing_from, &self.original);
    }

    /// Adopt the rewritten paragraph's current geometry before the next overlay frame.
    pub(crate) fn refresh_source(&mut self, block: &pdfcraft_engine::TextBlock, lines: Vec<pdfcraft_engine::TextLine>) {
        self.source_rect = block.rect.map(|v| v as f32);
        self.multiline = block.rows.len() > 1;
        self.source = Some((block.clone(), lines));
        let before = std::mem::replace(&mut self.text, block.editor_text.clone());
        if self.text != before {
            // A byte range of the old text: it may name other characters, or none, in the new.
            self.selection = None;
        }
        self.follow_rows(&before);
        self.original = self.text.clone();
    }

    fn current_formatting(&self, sources: &[(String, String)]) -> SelectionFormatting {
        SelectionFormatting {
            font_shown: self.font_shown(sources),
            size: Some(self.look.size),
            family: Some(self.look.family),
            bold: Some(self.look.bold),
            italic: Some(self.look.italic),
            color: Some(self.look.color),
        }
    }

    pub fn selection_formatting(&self, sources: &[(String, String)]) -> SelectionFormatting {
        let Some((block, lines)) = &self.source else {
            return self.current_formatting(sources);
        };

        let spans = block.edited_spans(&self.text);
        let matching_lines: Vec<&pdfcraft_engine::TextLine> = match &self.selection {
            Some(range) if range.start < range.end => {
                spans.iter().filter(|(span, _)| span.start < range.end && span.end > range.start).filter_map(|(_, idx)| lines.get(*idx)).collect()
            }
            _ => spans.iter().filter_map(|(_, idx)| lines.get(*idx)).collect(),
        };

        let Some(first) = matching_lines.first().copied() else {
            return self.current_formatting(sources);
        };

        let line_font = |l: &pdfcraft_engine::TextLine| -> String {
            if let Some((_, face)) = sources.iter().find(|(res, _)| res == &l.font) {
                return face.clone();
            }
            let base = l.base_font.as_str();
            let name = pdfcraft_engine::strip_subset_tag(base);
            if let Some((_, face)) = sources.iter().find(|(_, face)| face.as_str() == name) {
                return face.clone();
            }
            let look = look_of_line(l);
            let lower = name.to_ascii_lowercase();
            if ["times", "serif", "roman", "courier", "helvetica", "arial"].iter().any(|s| lower.contains(s)) {
                return look.family.label().to_string();
            }
            if name.is_empty() { tl!("Keep source fonts").to_string() } else { name.to_string() }
        };

        let first_font = line_font(first);
        let same_font = matching_lines.iter().skip(1).all(|l| line_font(l) == first_font);
        let font_shown = if same_font {
            if self.font_resource.is_some() || self.look.family != self.look0.family || self.family_chosen {
                self.font_shown(sources)
            } else {
                first_font
            }
        } else {
            tl!("Mixed").to_string()
        };

        let first_size = (first.size * 10.0).round() / 10.0;
        let same_size = matching_lines.iter().skip(1).all(|l| ((l.size * 10.0).round() / 10.0 - first_size).abs() < 0.05);
        let size = if same_size { Some(first_size) } else { None };

        let first_look = look_of_line(first);
        let same_family = matching_lines.iter().skip(1).all(|l| look_of_line(l).family == first_look.family);
        let family = if same_family { Some(first_look.family) } else { None };
        let same_bold = matching_lines.iter().skip(1).all(|l| l.bold == first.bold);
        let bold = if same_bold { Some(first.bold) } else { None };
        let same_italic = matching_lines.iter().skip(1).all(|l| l.italic == first.italic);
        let italic = if same_italic { Some(first.italic) } else { None };
        let same_color = matching_lines.iter().skip(1).all(|l| l.color == first.color);
        let color = if same_color { Some(first.color) } else { None };

        SelectionFormatting { font_shown, size, family, bold, italic, color }
    }

    pub(crate) fn sync_selection_look(&mut self, fmt: &SelectionFormatting) {
        if let Some(sz) = fmt.size {
            self.look.size = sz;
            self.look0.size = sz;
        } else {
            if self.look.size <= 0.0 {
                self.look.size = 12.0;
            }
            self.look0.size = self.look.size;
        }
        if let Some(f) = fmt.family {
            self.look.family = f;
            self.look0.family = f;
        }
        match fmt.bold {
            Some(b) => {
                self.look.bold = b;
                self.look0.bold = b;
            }
            None => {
                self.look.bold = false;
                self.look0.bold = false;
            }
        }
        match fmt.italic {
            Some(it) => {
                self.look.italic = it;
                self.look0.italic = it;
            }
            None => {
                self.look.italic = false;
                self.look0.italic = false;
            }
        }
        if let Some(c) = fmt.color {
            self.look.color = c;
            self.look0.color = c;
        }
    }

    /// The text changed from `before` through the box's own typing (typing, Undo, Redo): rows
    /// aligned on their own keep their alignment, and a row Undo brings back gets its own back.
    pub(crate) fn follow_typed_rows(&mut self, ctx: &egui::Context, before: &str, place: crate::panes::Place) {
        follow_typing(ctx, input_id(place), before, &self.text, &mut [&mut self.look.aligns, &mut self.look0.aligns]);
    }

    /// The text changed from `before`: rows aligned on their own keep their alignment.
    pub(crate) fn follow_rows(&mut self, before: &str) {
        for look in [&mut self.look, &mut self.look0] {
            look.aligns = pdfcraft_engine::paragraph_align::remap(before, &self.text, &look.aligns);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectionFormatting {
    pub font_shown: String,
    pub size: Option<f64>,
    pub family: Option<pdfcraft_engine::FontFamily>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub color: Option<[f64; 3]>,
}

fn look_of_line(l: &pdfcraft_engine::TextLine) -> pdfcraft_engine::AddedText {
    source_look(&l.base_font, l.size, l.color, l.bold, l.italic)
}

/// What a drag on a paragraph box takes hold of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grip {
    Move,
    /// An edge (screen left or right), dragged to rewrap the paragraph.
    Left,
    Right,
}

/// A paragraph box being dragged: moved, or (from a handle on its left or right edge) resized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockDrag {
    page: usize,
    block: usize,
    /// A paragraph index belongs to the document generation where the press was resolved.
    generation: u64,
    /// Where the drag started (screen).
    start: Pos2,
    grip: Grip,
}

/// How far an edge grip reaches either side of a box's edge (screen pixels). Wide enough to
/// catch without aiming for the hairline (#2203); a narrow box still keeps its middle for moving.
const EDGE_REACH: f32 = 8.0;
/// How far outside an open paragraph's frame the pointer still means the frame (screen pixels):
/// the frame's Move cursor shows, and a press there takes hold of the box (`object_ui`).
pub(crate) const FRAME_REACH: f32 = 8.0;

/// The rewrap handles drawn on a paragraph box's left and right edges (screen).
fn width_handles(b: Rect) -> [Rect; 2] {
    let h = (b.height() * 0.5).clamp(14.0, 28.0);
    [b.left(), b.right()].map(|x| Rect::from_center_size(Pos2::new(x, b.center().y), egui::vec2(7.0, h)))
}

/// The edge of `b` that `p` grabs: anywhere along its height, within reach of the line. A narrow
/// box keeps its middle for moving.
fn edge_at(b: Rect, p: Pos2) -> Option<Grip> {
    if p.y < b.top() - EDGE_REACH || p.y > b.bottom() + EDGE_REACH {
        return None;
    }
    let inside = EDGE_REACH.min(b.width() / 4.0);
    let (dl, dr) = (p.x - b.left(), b.right() - p.x);
    match (dl >= -EDGE_REACH && dl <= inside, dr >= -EDGE_REACH && dr <= inside) {
        (true, true) if dl <= dr => Some(Grip::Left),
        (_, true) => Some(Grip::Right),
        (true, false) => Some(Grip::Left),
        _ => None,
    }
}

/// The box under `p` and what it grabs there; edges only on an upright page. An edge reaches a
/// little above and below its box, so where that reach overlaps another box the one `p` is level
/// with wins: a press beside one paragraph's edge doesn't rewrap the paragraph just under it.
/// Among boxes equally near, the topmost.
fn grab_at(boxes: &[Rect], p: Pos2, upright: bool) -> Option<(usize, Grip)> {
    let mut best: Option<(f32, usize, Grip)> = None;
    for (i, b) in boxes.iter().enumerate().rev() {
        let grip = match upright.then(|| edge_at(*b, p)).flatten() {
            Some(g) => g,
            None if b.contains(p) => Grip::Move,
            None => continue,
        };
        // How far above or below the box `p` is: 0 within its height.
        let off = (b.top() - p.y).max(p.y - b.bottom()).max(0.0);
        if best.is_none_or(|(o, ..)| off < o) {
            best = Some((off, i, grip));
        }
    }
    best.map(|(_, i, g)| (i, g))
}

/// A paragraph's box on screen: its text rect (user space), with a little room around it.
fn paragraph_box(xf: &PageXform, info: &DocInfo, page: usize, rect: [f64; 4]) -> Rect {
    xf.user_rect(info, page, rect.map(|v| v as f32)).expand(2.0)
}

/// Whether `p` lies in the box of one of the paragraphs `lines` on `page`, its edge grips there
/// included, not beside it: where Add text acts on the page's own paragraphs.
pub(crate) fn over_paragraph(xf: &PageXform, info: &DocInfo, page: usize, lines: &[pdfcraft_engine::TextBlock], p: Pos2) -> bool {
    lines.iter().any(|l| paragraph_box(xf, info, page, l.rect).contains(p))
}

/// The width handles need the page upright (or upside down): on a quarter-turned page the
/// screen's horizontal is the paragraph's vertical.
fn has_edges(info: &DocInfo, page: usize) -> bool {
    info.pages.get(page).is_some_and(|p| p.rotation % 180 == 0)
}

/// What of the paragraphs `lines` lies under `p` on `page`: the one hit test of Edit text's
/// paragraphs, for the cursor and the press. A closed paragraph moves when dragged (the four-way
/// arrow says so; a click still opens it for typing) and its edge rewraps it, from a little way
/// outside the box (the handle is drawn half outside it, #2276); the open one takes the text
/// cursor. `active` is the paragraph open for typing on this page.
pub(crate) fn hit(xf: &PageXform, info: &DocInfo, page: usize, lines: &[pdfcraft_engine::TextBlock], active: Option<usize>, p: Pos2) -> Option<Hit> {
    if !xf.rect.contains(p) {
        return None;
    }
    let boxes: Vec<Rect> = lines.iter().map(|l| paragraph_box(xf, info, page, l.rect)).collect();
    // Edges rewrap closed paragraphs only: around the open one lies its frame (`object_ui`).
    let (block, grip) = grab_at(&boxes, p, has_edges(info, page) && active.is_none())?;
    let paragraph = |part| Owner::Paragraph { block, part };
    Some(match (active, grip) {
        (None, Grip::Left) => Hit::new(paragraph(ParagraphPart::Left), Layer::Handle, egui::CursorIcon::ResizeHorizontal),
        (None, Grip::Right) => Hit::new(paragraph(ParagraphPart::Right), Layer::Handle, egui::CursorIcon::ResizeHorizontal),
        (None, Grip::Move) => Hit::new(paragraph(ParagraphPart::Body), Layer::Text, egui::CursorIcon::Move),
        (Some(open), _) if open == block => Hit::new(paragraph(ParagraphPart::Body), Layer::Editor, egui::CursorIcon::Text),
        (Some(_), _) => Hit::new(paragraph(ParagraphPart::Body), Layer::Text, egui::CursorIcon::Text),
    })
}

pub(crate) fn user_box(xf: &PageXform, info: &DocInfo, page: usize, r: Rect) -> [f64; 4] {
    let p = &info.pages[page];
    let (a, b) = (xf.screen_to_view(r.min), xf.screen_to_view(r.max));
    let (u, v) = (p.view_to_user(a.0, a.1), p.view_to_user(b.0, b.1));
    [u[0].min(v[0]) as f64, u[1].min(v[1]) as f64, u[0].max(v[0]) as f64, u[1].max(v[1]) as f64]
}

/// Open `block` on `page` in the inline editor. The box's geometry and source are read from the
/// `lines` handed in, so callers pass the document's current inventory.
fn open_editor(cx: PageToolContext<'_>, lines: &[pdfcraft_engine::TextBlock], boxes: &[Rect], block: usize, view: &mut DocView) {
    let PageToolContext { ui, xf, doc, page, .. } = cx;
    view.pending_text_open = None;
    let (Some(l), Some(b)) = (lines.get(block), boxes.get(block)) else { return };
    view.image_selection = None;
    let source_lines = doc.text_lines(page);
    let text = l.editor_text.clone();
    // Screen pixels per point, from the box's width.
    let scale = (b.width() - 4.0) / ((l.rect[2] - l.rect[0]).max(1.0) as f32);
    // How far the editor box may grow: a single-line paragraph's rewrite grows to the
    // page's right edge, a multi-line paragraph rewraps to its own width.
    let right = xf.rect.right().min(view.viewport_rect().right()) - 6.0;
    let max_width = if l.rows.len() == 1 { (right - b.left()).max(b.width()) } else { b.width() };
    view.line_editor = Some(LineEditor {
        selection: None,
        font_resource: None,
        page,
        block,
        text: text.clone(),
        original: text,
        rect: *b,
        source_rect: l.rect.map(|v| v as f32),
        multiline: l.rows.len() > 1,
        source: Some((l.clone(), source_lines)),
        fonts: install_source_fonts(ui.ctx(), doc, page),
        overflow: false,
        max_width,
        size: (l.size as f32 * scale).clamp(8.0, 72.0),
        focus: true,
        had_focus: false,
        look: look_of(l),
        look0: look_of(l),
        family_chosen: false,
        size_chosen: false,
        extras: Extras::default(),
        extras0: Extras::default(),
        typing_from: None,
    });
}

/// What Edit text's paragraphs are told on one page.
#[derive(Clone, Copy)]
pub(crate) struct ParagraphCx<'a> {
    /// What the press the pages hold is to this page.
    pub press: PagePress,
    /// Edit text's input is open: no dialog, palette, other window or other tab has it.
    pub gate: bool,
    /// What a press under the pointer would take (the top hit there): the paragraph outlined.
    pub hover: Option<&'a Hit>,
    /// With Add text, the page's added content: a click on a paragraph finishes the text being
    /// typed before it opens the paragraph.
    pub adding: Option<&'a [pdfcraft_engine::Added]>,
}

/// Lines and their screen boxes for a page: the paragraph under the pointer is outlined, and the
/// press that is a paragraph's (`press`) moves it, rewraps it or, a click, opens it in the editor.
/// Returns `true` when the pointer was used.
pub(crate) fn page_input(cx: PageToolContext<'_>, lines: &[pdfcraft_engine::TextBlock], pcx: ParagraphCx<'_>, view: &mut DocView) -> bool {
    let PageToolContext { ui, resp, xf, ptr, page, doc, .. } = cx;
    let ParagraphCx { press, gate, hover, adding } = pcx;
    let info = &doc.info;
    if view.block_drag.is_some_and(|d| d.generation != doc.edit_generation()) {
        view.block_drag = None;
        return true;
    }
    let boxes: Vec<Rect> = lines.iter().map(|l| paragraph_box(xf, info, page, l.rect)).collect();
    // Outlines stay on their page, like its text (#3061).
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(xf.rect.expand(1.0)));
    let active = view.line_editor.as_ref().filter(|e| e.page == page).map(|e| e.block);
    for (i, b) in boxes.iter().enumerate() {
        if active != Some(i) {
            painter.rect_stroke(*b, CornerRadius::same(2), Stroke::new(0.75, ACCENT.gamma_multiply(0.35)), egui::StrokeKind::Outside);
        }
    }
    // A box a switching click asked for opens now, once the edit queued with that click has
    // applied and `lines`/`boxes` are the document's new inventory (#2022).
    if gate
        && active.is_none()
        && let Some((open_page, key)) = view.pending_text_open.clone()
        && open_page == page
    {
        // The edit may have regrouped the page: open the paragraph that was clicked, wherever it
        // is now, and none if it was merged into another (#2516).
        match key.find(lines, 0) {
            Some(block) => open_editor(cx, lines, &boxes, block, view),
            None => view.pending_text_open = None,
        }
    }
    // A click on grouped text prepared the page (`image_input`): the paragraph now at the point
    // clicked opens, and none if the point is on the group's artwork. A preparation that failed
    // left the page as it was, where no paragraph is at that point. It is taken either way.
    if let Some((_, [x, y])) = view.grouped_text_click.take_if(|(p, _)| *p == page)
        && gate
        && active.is_none()
        && let Some((block, _)) = grab_at(&boxes, user_to_screen(xf, info, page, x, y), false)
    {
        open_editor(cx, lines, &boxes, block, view);
    }
    if !gate {
        return false;
    }
    let upright = has_edges(info, page);
    let draw_handles = |b: Rect| {
        for h in width_handles(b) {
            painter.rect(h, CornerRadius::same(2), Color32::WHITE, Stroke::new(1.0, ACCENT), egui::StrokeKind::Middle);
        }
    };
    // A drag in progress: the box follows the pointer (or the grabbed edge does); releasing applies it.
    if let Some(d) = view.block_drag.filter(|d| d.page == page)
        && let (Some(b), Some(l)) = (boxes.get(d.block).copied(), lines.get(d.block))
    {
        let p = ptr.pos.unwrap_or(d.start);
        let dx = p.x - d.start.x;
        let preview = match d.grip {
            Grip::Move => b.translate(p - d.start),
            Grip::Left => Rect::from_min_max(Pos2::new((b.left() + dx).min(b.right() - 12.0), b.min.y), b.max),
            Grip::Right => Rect::from_min_max(b.min, Pos2::new((b.right() + dx).max(b.left() + 12.0), b.max.y)),
        };
        painter.rect_stroke(preview, CornerRadius::same(2), Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
        let resizing = d.grip != Grip::Move;
        if resizing {
            draw_handles(preview);
        }
        view.cursor.claim(Layer::Drag, if resizing { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::Move });
        if resp.drag_stopped() || !ptr.any_down {
            view.block_drag = None;
            let (from, to) = (user_box(xf, info, page, b), user_box(xf, info, page, preview));
            let edit = if resizing {
                // The new width in user space: the box's change, added to the paragraph's own. A
                // moved left side (in user space; on an upside-down page that's the screen's right
                // edge) moves the paragraph with it.
                let width = (l.rect[2] - l.rect[0]) + (to[2] - to[0]) - (from[2] - from[0]);
                let shift = to[0] - from[0];
                let style = pdfcraft_engine::BlockStyle { width: Some(width), offset: (shift != 0.0).then_some([shift, 0.0]), ..Default::default() };
                // `editor_text` has a newline for every visual row, so a wrap would come back as a
                // hard break and a wider box would not reflow it. `reflow_text` rejoins a wrap
                // and keeps a newline that was typed.
                Edit::EditTextBlock { page, block: d.block, text: l.reflow_text.clone(), style }
            } else {
                // Translation must keep the selected source operators. Rewriting unchanged text
                // would also remove unrelated paragraphs that now overlap this one (#594).
                Edit::MoveObjects {
                    page,
                    objects: vec![ObjectTarget { kind: ObjectKind::Text, index: d.block }],
                    offset: [to[0] - from[0], to[1] - from[1]],
                }
            };
            if preview != b {
                view.pending_edit = Some(edit);
            }
        }
        return true;
    }
    // Dragging a box (not while a paragraph is open for typing) moves it; from an edge, resizes it.
    // What it grabs is what the press was on ([`hit`]), not what is under the pointer now: egui
    // only calls it a drag once the pointer has moved a few pixels (or been held a moment), and a
    // quick flick has left the edge by then.
    let owner = match press {
        PagePress::Mine(Owner::Paragraph { block, part }) => Some((block, part)),
        _ => None,
    };
    // Add text holds text that can't be drawn where it is being typed (#125).
    let held_draft = adding.is_some() && view.content.blocked().is_some();
    if ptr.drag_started
        && active.is_none()
        && !held_draft
        && let Some(o) = ptr.origin
        && let Some((block, part)) = owner
    {
        let grip = match part {
            ParagraphPart::Body => Grip::Move,
            ParagraphPart::Left => Grip::Left,
            ParagraphPart::Right => Grip::Right,
        };
        view.block_drag = Some(BlockDrag { page, block, generation: doc.edit_generation(), start: o, grip });
        return true;
    }
    // The paragraph under the pointer, where a press would take it: outlined, with its width
    // handles when closed. Not while the objects selected together or added content being moved
    // hold the page.
    let hovered = match hover.map(|h| h.owner) {
        Some(Owner::Paragraph { block, .. }) => Some(block),
        _ => None,
    };
    if let Some(b) = hovered.filter(|_| !view.objects.holds(page) && !view.content.moving_on(page)).and_then(|h| boxes.get(h).map(|b| (h, *b))) {
        if active != Some(b.0) {
            painter.rect_stroke(b.1, CornerRadius::same(2), Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
        }
        if upright && active.is_none() {
            draw_handles(b.1);
        }
    }
    // A click opens the paragraph the press was on.
    let Some((hit, _)) = owner else { return hovered.is_some() };
    if resp.clicked() {
        // Add text: text that can't be drawn stays where it is being typed (#125), and text being
        // typed is finished first.
        if let Some(added) = adding {
            if view.content.hold_blocked() {
                return true;
            }
            if let Some(edit) = view.content.finish_draft(added) {
                view.pending_edit = Some(edit);
            }
            view.content.selected = None;
        }
        view.image_selection = None;
        // A click that moves the editor to another paragraph applies the open one first, so its
        // typed text is not dropped with the switched-away editor (#2022). A click on the open
        // box's own frame, which the editor's overlay does not cover, keeps it open instead.
        if let Some(open) = view.line_editor.as_ref().filter(|e| e.has_changes()) {
            if open.page == page && open.block == hit {
                return true;
            }
            if let Some(old) = view.line_editor.take() {
                let style = old.style();
                if old.text != old.original || style != pdfcraft_engine::BlockStyle::default() {
                    view.applied_editor = Some(old.clone());
                    view.pending_edit = Some(Edit::EditTextBlock { page: old.page, block: old.block, text: old.text, style });
                }
            }
            // The clicked box opens next frame, from the document with that edit applied.
            view.pending_text_open = lines.get(hit).map(|l| (page, pdfcraft_engine::BlockKey::of(l)));
            return true;
        }
        open_editor(cx, lines, &boxes, hit, view);
    }
    true
}

/// The inline editor; returns the edit once the text is applied.
pub(crate) fn overlay(ctx: &egui::Context, keys: PageKeys, view: &mut DocView, info: &DocInfo) -> Option<Edit> {
    let view_id = view.id;
    // Clicks outside the document (the Format text panel) keep the paragraph open, as does the
    // keyboard moving there. Where the click landed is `interact_pos`: `latest_pos` is gone on the
    // frame a tap lifts.
    let away = |p: egui::Pos2| !view.viewport_rect().contains(p);
    let outside = ctx.input(|i| i.pointer.interact_pos()).is_some_and(away) || focus_pos(ctx).is_some_and(away);
    let page = view.line_editor.as_ref()?.page;
    // Keyed by view: split view can show an editor in each pane.
    let key = view.view_key();
    let place = view.place();
    let xf = view.page_xform(page)?;
    let hold = view.objects.hold_editor;
    // An OCR word is invisible text over a scan: the textless background still shows the scanned
    // word, so the editor covers it, as the applied edit will.
    let background_ready = view
        .line_editor
        .as_ref()
        .is_some_and(|ed| view.text_background.ready(ed.page, ed.block) && !ed.source.as_ref().is_some_and(|(b, _)| b.invisible));
    // The measured extent of this paragraph's draft: a click straight from another paragraph
    // opens this one after the frame's measuring, which was the other one's.
    let draft_rect = view
        .line_editor
        .as_ref()
        .and_then(|ed| view.text_background.draft_rect_for(ed.page, ed.block))
        .map(|r| xf.user_rect(info, page, r.map(|v| v as f32)));
    // The editor draws on its page only, as the page itself does: a paragraph running past the
    // page's edge (a converter relying on the page clip) must not cover the next page or the
    // tool bars (#3061).
    let viewport = view.viewport_rect();
    let page_clip = xf.rect.intersect(viewport);
    let ed = view.line_editor.as_mut()?;
    if hold {
        ed.focus = true;
    }
    // Reproject the source box every frame. The page may have been zoomed, scrolled or rotated
    // while the format panel was open.
    // A font definition becomes active at the start of the next egui pass. Keep the source
    // raster visible until then instead of flashing an unrelated interface face.
    if !ctx.fonts(|f| ed.fonts.values().all(|name| f.definitions().families.contains_key(&FontFamily::Name(name.clone().into())))) {
        ctx.request_repaint();
        return None;
    }
    ed.rect = xf.user_rect(info, ed.page, ed.source_rect);
    ed.max_width = ed.rect.width();
    let scale = (ed.rect.width() / (ed.source_rect[2] - ed.source_rect[0]).abs().max(0.01)).max(0.01);
    ed.size = (ed.look.size as f32 * scale).clamp(1.0, 300.0);
    // egui draws the caret at least one row of this font tall. Each row already has its source
    // row's height, so the font stays below every row (half the region's smallest size): on a
    // 12 pt row under a 24 pt title the caret is as tall as the row, not the title.
    let smallest =
        ed.source.as_ref().and_then(|(block, lines)| block.lines.iter().filter_map(|i| lines.get(*i)).map(|l| l.size as f32).reduce(f32::min));
    let font = editor_font(&ed.look, smallest.map_or(ed.size, |s| (s * scale * 0.5).clamp(1.0, 300.0).min(ed.size)));
    let text_color = color32(ed.look.color);
    let block = ed.block;
    let mut done = None;
    let mut reveal = false;
    let mut look = None;
    // No fade-in: while it faded, the page's own copy of the text showed through the cover.
    // Over the page, so only where the document shows: a paragraph scrolled under the mode bar or
    // a side panel must leave them their pointer (#2755). It stays at its paragraph, cut off as
    // the page is, rather than pushed back into the window.
    egui::Area::new(egui::Id::unique(("edit-text-line", key)))
        .order(egui::Order::Foreground)
        .fade_in(false)
        .fixed_pos(Pos2::new(ed.rect.left(), ed.rect.top()))
        .constrain_to(viewport)
        .constrain(false)
        .show(ctx, |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(page_clip));
            // The editor opens at the source size. Once text changes, show the real draft
            // extent: single rows grow toward the page edge, paragraphs rewrap at their
            // source width, and the visible frame grows/shrinks vertically with their rows.
            // An OCR phrase (several stretched words on one scanned line) also grows rightwards.
            let single_run = ed.source.as_ref().is_some_and(|(block, _)| block.rows.len() == 1 || block.invisible);
            let width = if !ed.multiline && single_run && !ed.text.contains('\n') && ed.has_changes() {
                let job = editor_job(ui, ed, &ed.text, scale, f32::INFINITY);
                let natural = ui.fonts_mut(|f| f.layout_job(job)).size().x;
                natural.max(ed.size * 0.4).min((xf.rect.right() - ed.rect.left() - 36.0 * scale).max(ed.rect.width()))
            } else {
                ed.rect.width().max(1.0)
            };
            ui.set_width(width);
            let measured = editor_galley(ui, ed, &ed.text, scale, width);
            ed.overflow = ed.has_changes() && (measured.rect.width() > ed.rect.width() + 1.5 || measured.rect.height() > ed.rect.height() + 1.5);
            let visible = Rect::from_min_size(ed.rect.min, measured.rect.size().max(egui::vec2(1.0, 1.0)));
            // Unsupported source programs use an interface fallback whose glyph advances
            // can differ from the PDF's. Keep every visible row inside the growing frame.
            let live = draft_rect.map_or(visible, |bounds| bounds.union(visible));
            ui.painter().rect_stroke(live.expand(2.0), CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
            // On the frame, a press takes hold of the box (a click selects it, a drag moves it):
            // the pointer says so there, and reads as text inside. Once the text is changed, a
            // press there applies the typing like any click off the text instead of taking the
            // box (`object_ui` would drop the typing), so the pointer doesn't offer it then
            // (review on #4624).
            if !ed.has_changes()
                && let Some(p) = ui.input(|i| i.pointer.hover_pos())
                && ui.clip_rect().contains(p)
                && live.expand(2.0 + FRAME_REACH).contains(p)
                && !live.expand(2.0).contains(p)
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
            }
            // White, unless the text is too pale to read on white (white text from a coloured
            // banner, #913): then a dark box, with a caret that shows on it.
            let fill = if background_ready { Color32::TRANSPARENT } else { editor_fill(text_color) };
            // An unchanged paragraph is the page's own text: the editor draws only its frame and
            // caret over it. Once it changes, the textless page goes under the editor's text or,
            // until that background is ready, a cover over the entire source box, including lines
            // the draft no longer occupies. Shrinking the text must never expose a copy. Both are
            // decided after this frame's typing, in places kept under the editor's text.
            let cover = ui.painter().add(egui::Shape::Noop);
            let unchanged = !ed.has_changes();
            egui::Frame::NONE.show(ui, |ui| {
                // A textless PDF preview, or the page itself, keeps its real background through
                // the transparent editor. The temporary opaque cover uses the shared caret-contrast rule.
                ui.visuals_mut().text_cursor.stroke.color = if background_ready || unchanged {
                    if luminance(text_color) < 0.5 { Color32::BLACK } else { Color32::WHITE }
                } else {
                    caret_color_on(fill)
                };
                // An unchanged paragraph's galley is unseen over the page's own text. Selecting
                // part of it must not paint its stand-in glyphs over that text in the selection
                // colour (a serif paragraph turned sans when highlighted), nor hide it under an
                // opaque fill: the selection is a tint the page shows through.
                if unchanged {
                    let selection = &mut ui.visuals_mut().selection;
                    selection.stroke.color = Color32::TRANSPARENT;
                    selection.bg_fill = selection.bg_fill.gamma_multiply(0.45);
                }
                let rows = 1;
                let snapshot = ed.clone();
                let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
                    let galley = editor_galley(ui, &snapshot, text.as_str(), scale, wrap_width);
                    if snapshot.changes_with(text.as_str()) { galley } else { unseen(galley) }
                };
                let before = ed.text.clone();
                let input = input_id(place);
                ed.claim_typing(ui.ctx(), view_id, place);
                let output = egui::TextEdit::multiline(&mut ed.text)
                    .id(input)
                    .font(font.clone())
                    .margin(egui::Margin::ZERO)
                    .layouter(&mut layouter)
                    .text_color(text_color)
                    .frame(egui::Frame::NONE)
                    .desired_width(width)
                    .desired_rows(rows)
                    .show(ui);
                // The editor paints over the page with no visible label; name it for assistive tech.
                output.response.response.clone().accessible_name(tl!("Text editing line"));
                if ed.has_changes() {
                    if background_ready {
                        reveal = true;
                    } else {
                        ui.painter().set(cover, egui::Shape::rect_filled(ed.rect.expand(1.0).union(output.response.rect), CornerRadius::ZERO, fill));
                    }
                }
                look = Some((Arc::clone(&output.galley), output.galley_pos));
                narrow_double_click(ui, input, &ed.text, &output);
                let r = output.response;
                if ed.focus {
                    r.request_focus();
                    if let Some(range) = &ed.selection {
                        reselect(ui.ctx(), input, &ed.text, range);
                    }
                    ed.focus = false;
                } else if r.has_focus() {
                    if let Some(cursor) = output.cursor_range {
                        let selected = cursor.as_sorted_char_range();
                        let byte = |index| ed.text.char_indices().nth(index).map_or(ed.text.len(), |(at, _)| at);
                        ed.selection = (!selected.is_empty()).then(|| byte(selected.start.into())..byte(selected.end.into()));
                    } else {
                        ed.selection = None;
                    }
                }
                if r.changed() {
                    ed.follow_typed_rows(ctx, &before, place);
                    // The next pass measures the updated draft with the Apply/Save rewrite.
                    ctx.request_repaint();
                }
                let lost = lost_focus_once(&r, &mut ed.had_focus);
                let esc = keys.pressed(ctx, egui::Key::Escape);
                let apply = keys.pressed(ctx, egui::Key::Enter) && ui.input(|i| i.modifiers.command);
                if esc {
                    done = Some(false);
                } else if apply || (lost && !outside && !hold) {
                    done = Some(true);
                }
            });
        });
    if reveal {
        view.text_background.reveal();
    }
    if let Some((galley, pos)) = look {
        view.text_background.remember(&xf, page, block, galley, pos, text_color);
    }
    match done {
        Some(apply) => {
            let ed = view.line_editor.take()?;
            if !apply {
                // Esc leaves the text but not the paragraph: its box is selected, as in Acrobat,
                // so Delete removes it without aiming for the frame. The draft is discarded.
                view.objects.select_paragraph_next(ed.page, ed.block);
                return None;
            }
            view.applied_editor = Some(ed.clone());
            finished_edit(ed)
        }
        None => None,
    }
}

/// The edit a closed paragraph editor applies, or `None` when nothing in it changed.
pub(crate) fn finished_edit(ed: LineEditor) -> Option<Edit> {
    let style = ed.style();
    (ed.text != ed.original || style != pdfcraft_engine::BlockStyle::default()).then_some(Edit::EditTextBlock {
        page: ed.page,
        block: ed.block,
        text: ed.text,
        style,
    })
}

/// Where the keyboard is: the centre of the widget that has it. A text box that loses it to a
/// widget reached by Tab or a screen reader (the Format text panel's controls) judges where it
/// went by this, as it judges a click by where the click landed.
pub(crate) fn focus_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id)).map(|r| r.rect.center())
}

/// Whether an editor's text box `r` lost the keyboard this frame, given whether it `had` it at
/// the end of the editor's last frame (updated here). egui's `lost_focus` holds for a second
/// frame as well, and an editor that judges a click away by where the click landed must judge it
/// once, on the click's frame: on the next, a tap's position is gone (egui-winit sends
/// `PointerGone` with every touch release) and a mouse may have moved on.
pub(crate) fn lost_focus_once(r: &egui::Response, had: &mut bool) -> bool {
    let has = r.has_focus();
    std::mem::replace(had, has) && !has
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_double_click_at_a_word_edge_means_that_word() {
        let text = "Take a calm walk.";
        let word = |caret| word_at_caret(text, caret).map(|r| &text[r]);
        assert_eq!(word(5), Some("a"), "before the one-letter word, after a space");
        assert_eq!(word(6), Some("a"), "at the end of the one-letter word");
        assert_eq!(word(17), Some("walk."), "at the end of the text");
        assert_eq!(word(4), Some("Take"), "at the end of a word, before a space");
        assert_eq!(word(7), Some("calm"), "at the start of a longer word");
        assert_eq!(word(9), Some("calm"), "inside a word");
        assert_eq!(word(0), Some("Take"));
        assert_eq!(word(11), Some("calm"), "at the end of a word, before a space");
    }

    /// A live text box holding "Take a calm walk." in a context of its own, driven with real pointer
    /// events; `narrow` runs the correction after the box each pass, as the editor does.
    struct LiveBox {
        ctx: egui::Context,
        id: egui::Id,
        text: String,
        time: f64,
        narrow: bool,
        width: f32,
        /// Where each char starts and ends (on the row it is drawn on), as screen points.
        spans: Vec<(egui::Pos2, egui::Pos2)>,
    }

    impl LiveBox {
        fn new(narrow: bool) -> Self {
            Self::with_width(narrow, 300.0)
        }

        fn with_width(narrow: bool, width: f32) -> Self {
            Self::with_text(narrow, width, "Take a calm walk.")
        }

        fn with_text(narrow: bool, width: f32, text: &str) -> Self {
            use egui::text::{CCursor, CCursorRange};
            let mut live = LiveBox {
                ctx: egui::Context::default(),
                id: egui::Id::unique("paragraph"),
                text: text.to_owned(),
                time: 0.0,
                narrow,
                width,
                spans: Vec::new(),
            };
            live.pass(vec![], egui::Modifiers::NONE);
            // The caret at the start of the text, in a focused box, as after typing there.
            live.ctx.memory_mut(|m| m.request_focus(live.id));
            let mut state = egui::TextEdit::load_state(&live.ctx, live.id).unwrap_or_default();
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(0))));
            egui::TextEdit::store_state(&live.ctx, live.id, state);
            live
        }

        /// One frame with `events`; returns the selected text.
        fn pass(&mut self, mut events: Vec<egui::Event>, modifiers: egui::Modifiers) -> Option<String> {
            self.time += 0.05;
            events.insert(0, egui::Event::ModifiersChanged(modifiers));
            let input = egui::RawInput { events, time: Some(self.time), ..Default::default() };
            let (id, narrow, width, count) = (self.id, self.narrow, self.width, self.text.chars().count());
            let mut selection = None;
            let mut spans = Vec::new();
            let mut output = self.ctx.run_ui(input, |ui| {
                let out = egui::TextEdit::multiline(&mut self.text).id(id).desired_width(width).show(ui);
                let row = out.galley_pos.to_vec2();
                spans = (0..count)
                    .map(|i| {
                        let start = egui::text::CCursor { index: i.into(), prefer_next_row: true };
                        let end = egui::text::CCursor::new(i + 1);
                        (out.galley.pos_from_cursor(start).left_center() + row, out.galley.pos_from_cursor(end).left_center() + row)
                    })
                    .collect();
                if narrow {
                    narrow_double_click(ui, id, &self.text, &out);
                }
                selection = out.cursor_range;
            });
            output.textures_delta.clear();
            self.spans = spans;
            selection.map(|r| {
                let [start, end] = r.sorted_cursors().map(|c| usize::from(c.index));
                self.text[start..end].to_owned()
            })
        }

        /// A point `fraction` of the way through char `at`, on its row.
        fn point(&self, at: usize, fraction: f32) -> egui::Pos2 {
            let (from, to) = self.spans[at];
            egui::Pos2::new(from.x + fraction * (to.x - from.x), from.y)
        }

        fn click(&mut self, at: egui::Pos2, clicks: usize, modifiers: egui::Modifiers) -> Option<String> {
            let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers };
            // Long after the last click, so this one does not count as a repeat of it.
            self.time += 2.0;
            self.pass(vec![egui::Event::PointerMoved(at)], modifiers);
            for _ in 0..clicks {
                self.pass(vec![press(true)], modifiers);
                self.pass(vec![press(false)], modifiers);
            }
            self.pass(vec![], modifiers)
        }
    }

    /// A Shift-click on `at` whose press and release arrive in the same frame.
    fn tap(live: &mut LiveBox, at: egui::Pos2, modifiers: egui::Modifiers) -> Option<String> {
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers };
        live.time += 2.0;
        live.pass(vec![egui::Event::PointerMoved(at)], modifiers);
        live.pass(vec![press(true), press(false)], modifiers);
        live.pass(vec![], modifiers)
    }

    /// A Shift-click that is pressed and released within one frame still keeps the narrowed word.
    #[test]
    fn a_shift_click_pressed_and_released_in_one_frame_keeps_the_word() {
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        let mut live = LiveBox::new(true);
        let a = live.point(5, 0.2);
        assert_eq!(live.click(a, 2, egui::Modifiers::NONE).as_deref(), Some("a"));
        let before = live.point(2, 0.5);
        assert_eq!(tap(&mut live, before, shift).as_deref(), Some("ke a"));
    }

    /// A double-click in the blank space after a short row is not narrowed to the one-letter word
    /// that starts the row below, though the caret it hits is that word's start.
    #[test]
    fn a_double_click_in_the_blank_after_a_row_does_not_narrow_to_the_next_row() {
        let blank = |narrow| {
            let mut live = LiveBox::with_text(narrow, 22.0, "i W");
            assert!(live.spans[2].0.y > live.spans[0].0.y + 1.0, "W wraps below the i: {:?}", live.spans);
            let at = egui::Pos2::new(live.spans[2].0.x + 8.0, live.spans[0].0.y);
            live.click(at, 2, egui::Modifiers::NONE)
        };
        assert_eq!(blank(true), blank(false));
    }

    /// The narrowed-word anchor does not pass to the document the text box is next used for.
    #[test]
    fn the_narrowed_word_anchor_does_not_pass_to_another_document() {
        use pdfcraft_engine::DocId;
        let (ctx, id) = (egui::Context::default(), egui::Id::unique("box"));
        let mut from = None;
        forget_typing_before(&ctx, id, DocId(1), &mut from, "Take a calm walk.");
        ctx.data_mut(|d| d.insert_temp(id.with("narrowed word"), [5usize, 6, 5, 6]));
        forget_typing_before(&ctx, id, DocId(1), &mut from, "Take a calm walk.");
        assert!(ctx.data(|d| d.get_temp::<[usize; 4]>(id.with("narrowed word"))).is_some(), "the same document keeps it");
        forget_typing_before(&ctx, id, DocId(2), &mut from, "Take a calm walk.");
        assert!(ctx.data(|d| d.get_temp::<[usize; 4]>(id.with("narrowed word"))).is_none());
    }

    /// Double-click one fifth into the one-letter "a" of "Take a calm walk.", with `shift` held or
    /// not; `narrow` runs the correction. Returns the selected text.
    fn double_click_on_a(shift: bool, narrow: bool) -> String {
        let mut live = LiveBox::new(narrow);
        let modifiers = egui::Modifiers { shift, ..Default::default() };
        let at = live.point(5, 0.2);
        live.click(at, 2, modifiers).expect("the double-click leaves a selection")
    }

    /// The live text box: egui's double-click selects "Take a" at the left of the one-letter word;
    /// the correction narrows it to "a" (#7413).
    #[test]
    fn double_clicking_a_one_letter_word_selects_only_that_word() {
        assert_eq!(double_click_on_a(false, true), "a");
    }

    /// With Shift held egui extends the selection from the caret; the correction leaves that
    /// range alone instead of cutting it down to the word under the pointer.
    #[test]
    fn a_shift_double_click_keeps_the_extended_selection() {
        let extended = double_click_on_a(true, false);
        assert!(extended.len() > 1, "egui's own Shift-double-click extends past the word: {extended:?}");
        assert_eq!(double_click_on_a(true, true), extended);
    }

    /// egui keeps the double-clicked word as the anchor of a following Shift-click; after the
    /// narrowing a Shift-click before the word keeps "a" selected, and a Shift-double-click extends
    /// by words from it, as it does after egui's own double-click of "calm".
    #[test]
    fn a_shift_click_after_the_narrowed_double_click_keeps_the_word() {
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        let mut live = LiveBox::new(true);
        let a = live.point(5, 0.2);
        assert_eq!(live.click(a, 2, egui::Modifiers::NONE).as_deref(), Some("a"));
        let before = live.point(2, 0.5);
        assert_eq!(live.click(before, 1, shift).as_deref(), Some("ke a"), "before the word: it stays selected");
        // Again from a fresh double-click: after the word, and inside it.
        let mut live = LiveBox::new(true);
        live.click(a, 2, egui::Modifiers::NONE);
        let after = live.point(9, 0.5);
        assert_eq!(live.click(after, 1, shift).as_deref(), Some("a cal"), "after the word");
        let mut live = LiveBox::new(true);
        live.click(a, 2, egui::Modifiers::NONE);
        let inside = live.point(5, 0.8);
        assert_eq!(live.click(inside, 1, shift).as_deref(), Some("a"), "inside the word");
        // By words: a Shift-double-click before the word reaches back over "Take " and keeps "a".
        let mut live = LiveBox::new(true);
        live.click(a, 2, egui::Modifiers::NONE);
        let before = live.point(2, 0.5);
        assert_eq!(live.click(before, 2, shift).as_deref(), Some("Take a"));
        // egui's own double-click inside "calm", then the same Shift-click, as the model.
        let mut live = LiveBox::new(false);
        let calm = live.point(9, 0.5);
        assert_eq!(live.click(calm, 2, egui::Modifiers::NONE).as_deref(), Some("calm"));
        let before = live.point(2, 0.5);
        assert_eq!(live.click(before, 1, shift).as_deref(), Some("ke a calm"));
    }

    /// Double-click char `word_at` of "Take a calm walk.", then Shift-click (`clicks` 1) or
    /// Shift-double-click (2) each `(char, clicks)` in turn in the same box; the selected text after
    /// each. `narrow` runs the correction; without it this is egui's own behaviour.
    fn shift_sequence(narrow: bool, word_at: usize, steps: &[(usize, usize)]) -> Vec<String> {
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        let mut live = LiveBox::new(narrow);
        let at = live.point(word_at, 0.2);
        let mut shown = vec![live.click(at, 2, egui::Modifiers::NONE).expect("a selection")];
        for &(char_at, clicks) in steps {
            let at = live.point(char_at, 0.5);
            shown.push(live.click(at, clicks, shift).expect("a selection"));
        }
        shown
    }

    /// The word stays the anchor through a whole run of Shift-clicks: before it, after it, before
    /// it again, in one box, as after egui's own double-click of "calm" ("a" narrowed from egui's
    /// "Take a" against egui's own "calm", with the same pointer positions).
    #[test]
    fn shift_clicks_in_a_row_keep_extending_from_the_narrowed_word() {
        let steps = [(2, 1), (14, 1), (0, 1), (14, 1)];
        let narrowed = shift_sequence(true, 5, &steps);
        assert_eq!(narrowed, ["a", "ke a", "a calm wa", "Take a", "a calm wa"]);
        // egui's own, on a word it selects right: the same run around "calm" at 9.
        let egui = shift_sequence(false, 9, &steps);
        assert_eq!(egui, ["calm", "ke a calm", "calm wa", "Take a calm", "calm wa"]);
    }

    /// Shift-double-clicks in a row extend by words from the narrowed word, in both directions.
    #[test]
    fn shift_double_clicks_in_a_row_extend_by_words_from_the_narrowed_word() {
        let narrowed = shift_sequence(true, 5, &[(2, 2), (14, 2), (0, 2)]);
        let egui = shift_sequence(false, 9, &[(2, 2), (14, 2), (0, 2)]);
        assert_eq!(narrowed, ["a", "Take a", "a calm walk", "Take a"]);
        assert_eq!(egui, ["calm", "Take a calm", "calm walk", "Take a calm"]);
    }

    /// A word that starts a wrapped row is narrowed too: the hit there is the end of the row above.
    #[test]
    fn a_one_letter_word_that_starts_a_wrapped_row_is_narrowed() {
        // Narrow enough that "Take a" does not fit on one row.
        let mut live = LiveBox::with_width(true, 34.0);
        assert!(live.spans[5].0.y > live.spans[4].0.y + 1.0, "the box wraps before the a: {:?}", live.spans);
        let at = live.point(5, 0.2);
        assert_eq!(live.click(at, 2, egui::Modifiers::NONE).as_deref(), Some("a"));
    }

    /// A key that moves the selection ends the word anchor: the next Shift-click extends from the
    /// selection's own end, as egui does, not from the word.
    #[test]
    fn a_key_that_moves_the_selection_ends_the_word_anchor() {
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        let mut live = LiveBox::new(true);
        let a = live.point(5, 0.2);
        assert_eq!(live.click(a, 2, egui::Modifiers::NONE).as_deref(), Some("a"));
        let right = |pressed| egui::Event::Key { key: egui::Key::ArrowRight, physical_key: None, pressed, repeat: false, modifiers: shift };
        live.pass(vec![right(true)], shift);
        assert_eq!(live.pass(vec![right(false)], shift).as_deref(), Some("a "), "Shift+Right extends by a char");
        let before = live.point(2, 0.5);
        assert_eq!(live.click(before, 1, shift).as_deref(), Some("ke "), "from the selection's own start");
    }

    /// A double-click on the space after "Take" is left to egui, from either half of the space;
    /// the caret it hits ends "Take", but the pointer is not over the word.
    #[test]
    fn a_double_click_on_a_space_is_left_to_egui() {
        for fraction in [0.1, 0.4, 0.6, 0.9] {
            let space = |narrow| {
                let mut live = LiveBox::new(narrow);
                let at = live.point(4, fraction);
                live.click(at, 2, egui::Modifiers::NONE)
            };
            assert_eq!(space(true), space(false), "{fraction} of the way through the space");
        }
        // Over the glyphs of the word before and after it, the word is what is selected.
        let mut live = LiveBox::new(true);
        let at = live.point(3, 0.8);
        assert_eq!(live.click(at, 2, egui::Modifiers::NONE).as_deref(), Some("Take"));
    }

    /// Where an edge's reach above or below its box overlaps the next paragraph's edge, the
    /// paragraph the pointer is level with takes the press, whichever is drawn on top (the play
    /// session "resize grown neighbor" at 50% zoom).
    #[test]
    fn an_edge_goes_to_the_box_the_pointer_is_level_with() {
        let upper = Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(200.0, 140.0));
        let lower = Rect::from_min_max(Pos2::new(100.0, 146.0), Pos2::new(201.0, 160.0));
        // Inside the upper box's height, 6 px above the lower one: within both edges' reach.
        let p = Pos2::new(199.0, 140.0);
        assert_eq!(edge_at(lower, p), Some(Grip::Right), "the lower edge reaches up to p");
        assert_eq!(grab_at(&[upper, lower], p, true), Some((0, Grip::Right)));
        assert_eq!(grab_at(&[lower, upper], p, true), Some((1, Grip::Right)));
        // Level with neither, 4 px below one and 2 px above the other: the nearer box.
        assert_eq!(grab_at(&[lower, upper], Pos2::new(199.0, 144.0), true), Some((0, Grip::Right)));
        // Overlapping boxes both level with p: the topmost, as before.
        assert_eq!(grab_at(&[upper, upper.translate(egui::vec2(5.0, 0.0))], upper.center(), true), Some((1, Grip::Move)));
        // A body under p beats an edge reaching over from a box above or below it.
        let body = Rect::from_min_max(Pos2::new(150.0, 146.0), Pos2::new(260.0, 160.0));
        assert_eq!(grab_at(&[body, upper], Pos2::new(205.0, 147.0), true), Some((0, Grip::Move)));
    }

    /// Formatting applied restarts the rows' alignments by text with the box's history: a Redo
    /// past the format can't bring back an alignment from before it (FredApps on bfc6f8b1).
    #[test]
    fn formatting_applied_restarts_the_alignments_by_text() {
        use pdfcraft_engine::{DocId, TextAlign::Right};
        let (ctx, id) = (egui::Context::default(), egui::Id::unique("box"));
        let mut from = None;
        forget_typing_before(&ctx, id, DocId(1), &mut from, "A\nB");
        let mut aligns = vec![None, Some(Right)];
        follow_typing(&ctx, id, "A\nB", "A", &mut [&mut aligns]);
        // Formatting applied: the page now has "A", and the box's record of its text is reset.
        from = None;
        forget_typing_before(&ctx, id, DocId(1), &mut from, "A");
        follow_typing(&ctx, id, "A", "A\nB", &mut [&mut aligns]);
        assert_eq!(aligns, pdfcraft_engine::paragraph_align::remap("A", "A\nB", &[None]), "the row before the format doesn't come back aligned");
    }

    /// The rows' alignments by text go aside with the box's history when another tab's box takes
    /// the text box, and come back with it: Undo after the trip still restores a deleted row's
    /// alignment, while the other tab's box starts with none.
    #[test]
    fn alignments_by_text_survive_a_trip_to_another_tab() {
        use pdfcraft_engine::{DocId, TextAlign::Right};
        let (ctx, id) = (egui::Context::default(), egui::Id::unique("box"));
        let (mut a_from, mut b_from) = (None, None);
        forget_typing_before(&ctx, id, DocId(1), &mut a_from, "A\nB");
        let mut aligns = vec![None, Some(Right)];
        follow_typing(&ctx, id, "A\nB", "A", &mut [&mut aligns]);
        forget_typing_before(&ctx, id, DocId(2), &mut b_from, "A\nB");
        let mut other = vec![None, None];
        follow_typing(&ctx, id, "A\nB", "A", &mut [&mut other]);
        follow_typing(&ctx, id, "A", "A\nB", &mut [&mut other]);
        assert_eq!(other, [None, None], "tab B's box gets back its own alignments, not tab A's");
        forget_typing_before(&ctx, id, DocId(1), &mut a_from, "A\nB");
        follow_typing(&ctx, id, "A", "A\nB", &mut [&mut aligns]);
        assert_eq!(aligns, [None, Some(Right)], "back in tab A, Undo brings the row back right-aligned");
    }

    /// Undo back to a text the box showed brings its rows' alignments back, unless an alignment
    /// was picked since: then the older alignments are stale, and the edit is followed instead, so
    /// Undo never takes back the pick.
    #[test]
    fn typing_history_restores_alignments_unless_one_was_picked_since() {
        use pdfcraft_engine::TextAlign::{Center, Right};
        let (ctx, id) = (egui::Context::default(), egui::Id::unique("box"));
        let mut aligns = vec![None, Some(Center), Some(Right)];
        follow_typing(
            &ctx,
            id,
            "A
B
C",
            "A
C",
            &mut [&mut aligns],
        );
        assert_eq!(aligns, [None, Some(Right)], "the deleted row's entry goes");
        follow_typing(
            &ctx,
            id,
            "A
C",
            "A
B
C",
            &mut [&mut aligns],
        );
        assert_eq!(aligns, [None, Some(Center), Some(Right)], "Undo brings it back");
        follow_typing(
            &ctx,
            id,
            "A
B
C",
            "A
C",
            &mut [&mut aligns],
        );
        // An alignment picked for the first row, with no typing.
        aligns = vec![Some(Right), Some(Right)];
        let picked = aligns.clone();
        follow_typing(
            &ctx,
            id,
            "A
C",
            "A
B
C",
            &mut [&mut aligns],
        );
        assert_eq!(
            aligns,
            pdfcraft_engine::paragraph_align::remap(
                "A
C", "A
B
C", &picked
            ),
            "the pick stays"
        );
    }

    #[test]
    fn the_rotate_knob_moves_below_when_there_is_no_room_above() {
        let view = Rect::from_min_max(Pos2::new(0.0, 100.0), Pos2::new(500.0, 500.0));
        let roomy = Rect::from_min_max(Pos2::new(100.0, 200.0), Pos2::new(200.0, 250.0));
        assert_eq!(rotate_knob(roomy, view).0, Pos2::new(150.0, 178.0));
        let at_top = Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(200.0, 150.0));
        let (knob, stem) = rotate_knob(at_top, view);
        assert_eq!((knob, stem), (Pos2::new(150.0, 172.0), Pos2::new(150.0, 150.0)));
    }

    #[test]
    fn the_rotate_knob_measures_clockwise_degrees_from_straight_up() {
        let c = Pos2::new(100.0, 100.0);
        let at = |x: f32, y: f32| turn_degrees(c, Pos2::new(x, y), false);
        assert!(at(100.0, 0.0).abs() < 1e-3);
        assert!((at(200.0, 100.0) - 90.0).abs() < 1e-3, "right is a clockwise quarter");
        assert!((at(0.0, 100.0) + 90.0).abs() < 1e-3);
        assert!((at(100.0, 200.0).abs() - 180.0).abs() < 1e-3);
        // Shift rounds to 15 degrees; the centre and non-finite points turn nothing.
        assert_eq!(turn_degrees(c, Pos2::new(180.0, 50.0), true) % 15.0, 0.0);
        assert_eq!(at(101.0, 101.0), 0.0);
        assert_eq!(at(f32::NAN, 5.0), 0.0);
    }

    #[test]
    fn the_rotate_knob_measures_clockwise_delta_from_drag_start() {
        let c = Pos2::new(100.0, 100.0);
        // Start dragging at 45° clockwise from top:
        let r = 50.0_f32;
        let start = c + egui::vec2(r * 45.0_f32.to_radians().sin(), -r * 45.0_f32.to_radians().cos());
        // At start position, delta is 0:
        assert!(turn_degrees_delta(c, start, start, false).abs() < 1e-3);
        // Move by +15° (to 60°):
        let p_60 = c + egui::vec2(r * 60.0_f32.to_radians().sin(), -r * 60.0_f32.to_radians().cos());
        assert!((turn_degrees_delta(c, start, p_60, false) - 15.0).abs() < 1e-2);
        // Move by -10° (to 35°):
        let p_35 = c + egui::vec2(r * 35.0_f32.to_radians().sin(), -r * 35.0_f32.to_radians().cos());
        assert!((turn_degrees_delta(c, start, p_35, false) + 10.0).abs() < 1e-2);
    }

    /// An app with one text document open, and an editor on its first paragraph.
    fn preview_app() -> (crate::PdfCraftApp, LineEditor) {
        let mut app = crate::PdfCraftApp::new();
        let bytes = app.session.create_from_text("Preview", "Preview text").unwrap();
        app.open_bytes("preview.pdf", None, bytes.as_ref().clone()).unwrap();
        let doc = app.session.get(app.views[0].id).unwrap();
        let block = doc.text_blocks(0).remove(0);
        let ed = LineEditor {
            selection: None,
            font_resource: None,
            page: 0,
            block: 0,
            text: block.text.clone(),
            original: block.text.clone(),
            rect: Rect::NOTHING,
            source_rect: block.rect.map(|v| v as f32),
            multiline: block.rows.len() > 1,
            source: None,
            fonts: BTreeMap::new(),
            overflow: false,
            max_width: 400.0,
            size: block.size as f32,
            focus: true,
            had_focus: false,
            look: look_of(&block),
            look0: look_of(&block),
            family_chosen: false,
            size_chosen: false,
            extras: Extras::default(),
            extras0: Extras::default(),
            typing_from: None,
        };
        (app, ed)
    }

    #[test]
    fn review_committing_the_shown_size_applies_it_to_a_mixed_selection() {
        let (_app, mut ed) = preview_app();
        ed.selection = Some(0..2);
        let mut fmt = ed.current_formatting(&[]);
        fmt.size = None;
        ed.sync_selection_look(&fmt);
        let shown = ed.look.size;
        ed.choose_size(shown);
        assert!(ed.has_changes());
        assert_eq!(ed.style().ranges[0].size, Some(shown));
        ed.applied();
        assert!(ed.style().ranges.is_empty());
    }

    #[test]
    fn review_a_caret_only_script_formats_the_whole_draft() {
        let mut text = "x2".to_string();
        let mut caret = Some(1..1);
        assert!(toggle_script(&mut text, &mut caret, pdfcraft_engine::text_script::Script::Super));
        assert_eq!(text, "ˣ²");
        assert_eq!(caret, None);
    }

    #[test]
    fn picking_the_family_the_font_name_reads_as_still_changes_the_font() {
        // A Calibri paragraph reads as Helvetica; picking Helvetica from the list used to leave
        // `look` equal to what the paragraph had, so no family change was sent and Calibri stayed.
        let (_app, mut ed) = preview_app();
        let family = ed.look0.family;
        assert_eq!(ed.style().family, None, "nothing chosen yet");
        ed.choose_family(family);
        assert_eq!(ed.style().family, Some((family, ed.look.bold, ed.look.italic)));
        assert!(ed.has_changes());
        assert_eq!(ed.font_shown(&[]), family.label());
        ed.keep_source_font();
        assert_eq!(ed.style().family, None, "Keep source fonts withdraws the choice");
        ed.choose_family(family);
        ed.applied();
        assert_eq!(ed.style().family, None, "an applied choice is the paragraph's font now");
    }

    #[test]
    fn a_loaded_font_is_applied_to_the_paragraph_being_edited() {
        // Loading a font used to only register it on the page: the paragraph kept its source
        // face until the font was found again in the document-font menu (#2591).
        let (mut app, ed) = preview_app();
        app.views[0].line_editor = Some(ed);
        let inter = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf").to_vec();
        app.use_files(crate::files::FilePurpose::TextFont { page: 0 }, vec![("Inter-Regular.ttf".into(), inter)]);
        let doc = app.session.get(app.views[0].id).unwrap();
        let lines = doc.text_lines(0);
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), "Preview text");
        assert!(lines.iter().all(|l| l.base_font == "Inter-Regular"), "{:?}", lines.iter().map(|l| &l.base_font).collect::<Vec<_>>());
        assert!(app.views[0].line_editor.is_some(), "the paragraph stays open for editing");
    }

    #[test]
    fn save_applies_the_paragraph_open_in_the_editor() {
        // Save wrote the file without the paragraph still open in the editor (#3496).
        let (mut app, mut ed) = preview_app();
        ed.text = "Text preview".into();
        app.views[0].line_editor = Some(ed);
        let dir = std::env::temp_dir().join(format!("pdfcraft-ui-open-paragraph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        app.save_override = Some(dir.join("out.pdf").to_string_lossy().into_owned());

        assert!(app.save_active(crate::SaveTarget::InPlace));
        assert!(app.views[0].line_editor.is_none());
        let doc = app.session.get(app.views[0].id).unwrap();
        assert_eq!(doc.text_lines(0).iter().map(|l| l.text.as_str()).collect::<String>(), "Text preview");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_text_box_does_not_lose_the_open_paragraph() {
        let (mut app, mut ed) = preview_app();
        ed.text = "Text preview".into();
        app.views[0].line_editor = Some(ed);
        let draft = crate::content_ui::TextDraft {
            page: 9,
            index: None,
            rect: [20.0, 20.0, 120.0, 40.0],
            text: "Typed text".into(),
            style: crate::content_ui::default_style(),
            focus: true,
            selection: None,
            typing_from: None,
        };
        app.views[0].content.draft = Some(draft.clone());

        assert!(!app.commit_typing_in(app.views[0].id));
        assert_eq!(app.views[0].content.draft, Some(draft), "the refused text box stays open");
        assert!(app.views[0].line_editor.is_none(), "the paragraph was applied");
        let doc = app.session.get(app.views[0].id).unwrap();
        assert_eq!(doc.text_lines(0).iter().map(|l| l.text.as_str()).collect::<String>(), "Text preview");
    }

    /// Typing in a recognised phrase whose words OCR sized at 15, 9 and 11 pt lays every word out
    /// at the phrase's one size, as the engine redraws it (#3313); each word used its own.
    #[test]
    fn typing_in_an_ocr_phrase_lays_every_word_out_at_one_size() {
        use pdfcraft_engine::ocr::PlacedWord;
        // A word box as OCR draws it around capitals: the cap height of a `size` pt word.
        let word = |text: &str, x0: f64, x1: f64, size: f64| PlacedWord {
            text: text.into(),
            origin: [x0, 70.0],
            across: [x1 - x0, 0.0],
            up: [0.0, 0.718 * size],
            confidence: None,
            source: "ocrs".into(),
        };
        let mut app = crate::PdfCraftApp::new();
        let png = pdfcraft_engine::export::encode_png(200, 100, &[235u8; 200 * 100 * 4]).unwrap();
        let bytes = app.session.create_from_images(&[("scan.png".into(), png)]).unwrap();
        app.open_bytes("scan.pdf", None, bytes.as_ref().clone()).unwrap();
        let id = app.views[0].id;
        let words = vec![word("JOHN", 20.0, 45.0, 15.0), word("SMITH", 49.0, 78.0, 9.0), word("EXAMPLE", 82.0, 124.0, 11.0)];
        app.session.apply(id, Edit::AddOcrText { page: 0, words }).unwrap();
        let doc = app.session.get(id).unwrap();
        let block = doc.text_blocks(0).remove(0);
        assert!(block.invisible && block.lines.len() == 3, "one phrase of three recognised words: {block:?}");
        let (_, mut ed) = preview_app();
        ed.text = format!("{}S", block.editor_text);
        ed.original = block.editor_text.clone();
        ed.look = look_of(&block);
        ed.look0 = look_of(&block);
        ed.source = Some((block, doc.text_lines(0)));

        let ctx = egui::Context::default();
        let mut sizes = Vec::new();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = editor_job(ui, &ed, &ed.text, 1.0, f32::INFINITY);
            sizes = job.sections.iter().map(|section| section.format.font_id.size).collect();
        })
        .textures_delta
        .clear();
        assert!(sizes.len() >= 3, "a section for each word: {sizes:?}");
        assert!(sizes.iter().all(|size| (size - 11.0).abs() < 0.05), "every word at the phrase's 11 pt: {sizes:?}");
    }

    #[test]
    fn a_refused_paragraph_stays_open_with_its_text() {
        let (mut app, mut ed) = preview_app();
        ed.text = "Text preview".into();
        ed.page = 9;
        app.views[0].line_editor = Some(ed.clone());

        assert!(!app.commit_typing_in(app.views[0].id));
        assert_eq!(app.views[0].line_editor, Some(ed), "the paragraph keeps its unsaved text");
    }

    #[test]
    fn text_background_is_cached_without_editing_and_released_on_cancel() {
        let (app, ed) = preview_app();
        let doc = app.session.get(app.views[0].id).unwrap();
        let source = doc.bytes.clone();
        let generation = doc.edit_generation();
        let ctx = egui::Context::default();
        let mut background = TextBackground::default();
        for _ in 0..100 {
            background.prepare(&ctx, doc, Some(&ed), None, 1.0);
            if background.ready(0, 0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(background.ready(0, 0));
        assert!(!background.pending());
        let texture = background.texture.as_ref().unwrap().id();
        background.prepare(&ctx, doc, Some(&ed), None, 1.0);
        assert_eq!(background.texture.as_ref().unwrap().id(), texture);
        assert_eq!(doc.bytes, source);
        assert_eq!(doc.edit_generation(), generation);
        assert!(!doc.dirty && doc.can_undo().is_none());
        background.prepare(&ctx, doc, None, None, 1.0);
        assert!(background.texture.is_none() && background.renderer.is_none());
    }

    #[test]
    fn the_editing_background_stays_sharp_on_a_zoomed_page() {
        // While a paragraph is open, the page around it is drawn from this one raster: it is
        // asked for at the tiling threshold, not the 2048-pixel backdrop side, which went soft
        // beyond about 1.2x on a Retina display.
        let (app, ed) = preview_app();
        let doc = app.session.get(app.views[0].id).unwrap();
        let page = &doc.info.pages[0];
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput { max_texture_side: Some(16384), ..Default::default() });
        let mut background = TextBackground::default();
        background.prepare(&ctx, doc, Some(&ed), None, 8.0);
        let expected = 8.0f32.min(crate::canvas::TILE_THRESHOLD / page.width.max(page.height));
        assert!(expected > crate::canvas::BASE_SIDE / page.width.max(page.height), "the test page needs the larger side");
        assert_eq!(background.tag, Some((expected * 1000.0) as u64));
        // A GPU that can't hold such a texture caps it.
        ctx.end_pass().textures_delta.clear();
        ctx.begin_pass(egui::RawInput { max_texture_side: Some(2048), ..Default::default() });
        let mut small = TextBackground::default();
        small.prepare(&ctx, doc, Some(&ed), None, 8.0);
        assert_eq!(small.tag, Some(((2048.0 / page.width.max(page.height)) * 1000.0) as u64));
        ctx.end_pass().textures_delta.clear();
    }

    #[test]
    fn pale_text_is_edited_on_a_dark_box() {
        // #913: white text from a coloured banner was typed onto a white box and vanished.
        for pale in [Color32::WHITE, Color32::from_rgb(0xFF, 0xFF, 0x00), Color32::from_gray(0xE0), Color32::from_rgb(0xFF, 0xA5, 0x00)] {
            assert_eq!(editor_fill(pale), DARK_EDITOR_FILL, "{pale:?}");
            assert!(contrast(pale, editor_fill(pale)) >= 4.5, "{pale:?} is readable");
        }
        // Ordinary dark or saturated text keeps the white box.
        for dark in [Color32::BLACK, Color32::from_rgb(0x1F, 0x2A, 0x6B), Color32::from_rgb(0xC0, 0x00, 0x00), Color32::from_gray(0x60)] {
            assert_eq!(editor_fill(dark), Color32::WHITE, "{dark:?}");
        }
    }

    #[test]
    fn caret_contrasts_with_its_fill() {
        assert_eq!(caret_color_on(Color32::WHITE), Color32::BLACK);
        assert_eq!(caret_color_on(Color32::from_rgb(0xFF, 0xFF, 0xF4)), Color32::BLACK);
        assert_eq!(caret_color_on(Color32::BLACK), Color32::WHITE);
        assert_eq!(caret_color_on(DARK_EDITOR_FILL), Color32::WHITE);
    }

    #[test]
    fn superscript_and_subscript_conversion() {
        use pdfcraft_engine::text_script::Script;
        let (mut text, mut selection) = ("E = mc2".to_string(), Some(6..7));
        assert!(toggle_script(&mut text, &mut selection, Script::Super));
        assert_eq!((text.as_str(), selection.clone()), ("E = mc²", Some(6..8)), "the selection follows the converted character");
        assert!(toggle_script(&mut text, &mut selection, Script::Super));
        assert_eq!((text.as_str(), selection), ("E = mc2", Some(6..7)));

        // No selection: the whole text. H and O have no subscript form, and it still toggles back.
        let (mut text, mut none) = ("H2O".to_string(), None);
        assert!(toggle_script(&mut text, &mut none, Script::Sub));
        assert_eq!((text.as_str(), none.clone()), ("H₂O", None));
        assert!(toggle_script(&mut text, &mut none, Script::Sub));
        assert_eq!(text, "H2O");
    }

    #[test]
    fn value_based_line_spacing_in_style() {
        let ed = LineEditor {
            page: 0,
            block: 0,
            text: "Paragraph text".to_string(),
            original: "Paragraph text".to_string(),
            rect: egui::Rect::ZERO,
            source_rect: [0.0; 4],
            max_width: 100.0,
            overflow: false,
            selection: None,
            font_resource: None,
            fonts: std::collections::BTreeMap::new(),
            source: None,
            multiline: false,
            size: 12.0,
            family_chosen: false,
            size_chosen: false,
            focus: true,
            had_focus: false,
            look: pdfcraft_engine::AddedText::default(),
            look0: pdfcraft_engine::AddedText::default(),
            extras: Extras { line_spacing: 1.33, underline: true, ..Default::default() },
            extras0: Extras::default(),
            typing_from: None,
        };
        let style = ed.style();
        assert_eq!(style.line_spacing, Some(1.33));
        assert_eq!(style.underline, Some(true));
    }
}
