//! Edit a PDF ▸ Edit text & images: boxes around the paragraphs and images already on the page.
//! Click a paragraph to edit it in place (⌘Enter or clicking away applies and rewraps it to the
//! box, Esc cancels); drag it to move it, or drag the handle on its left or right edge to rewrap
//! it to a new width. Click an image to select it: drag to move, drag a corner to resize (keeping
//! its proportions), right-click for rotate, flip, replace, save and delete; Delete removes it.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use craft::behavior::edit_history::{EditHistory, Step};
use craft::behavior::geometry::quad::{BoxAllow, BoxHandles, BoxRules, BoxState, ScaleRule};
use craft::behavior::hit::HitShape;
use craft::behavior::kurbo::{BezPath, Point, Rect as DocRect, Shape as _};
use craft::behavior::value::{EditEvent, GestureId, Mixed};
use craft::behavior::{CanvasView, CursorIcon, PressPhase};
use craft::draw::ui::transform::TransformBox;
use craft::pages::ui::{PageBody, PlacedPage};
use craft::text::ui::editor::{CaretLine, EditKind, EditorEnd, HostLine, HostRun, TextEditor, TextEditorHost, TextSelection};
use craft::ui::canvas_controls::SelectionOutline;
use craft::ui::contract::{Bind, Props as _, Show as _};
use craft::ui::fonts::FontRef;
use craft::ui::runtime::focus::FocusRequest;
use egui::epaint::text::CharIndex;
use egui::{Pos2, Rect, Vec2, pos2, vec2};
use pdfcraft_engine::{Edit, ObjectKind, ObjectTarget};
use pdfcraft_render::DocInfo;

use crate::canvas::page_parts::{Owner, PageLayer, PagePart, PagePartsOutput, PagePress};
use crate::canvas::{PageGeometry, PageSession, PageXform};

// PLAN: at the base `image.rs` imports `ACCENT`, `Color32`, `Stroke` and `PagePointer` from here; they
// leave with the old overlay (pdf-edit-w9's rewrite takes theme roles instead).
mod background;
mod draft;
mod image;
mod layout;

pub(crate) use background::TextBackground;
use draft::{ParagraphCover, ParagraphText};
pub use image::{CropMode, ImageAction, ImageSelection};
pub(crate) use image::{PictureMenu, delete_picture, picture_context, picture_controls, picture_menu, picture_parts, picture_press, user_to_screen};
pub(crate) use layout::editor_font;
#[cfg(test)]
use layout::{DARK_EDITOR_FILL, contrast, editor_fill, editor_job};
use layout::{color32, editor_galley, look_of, source_look};

/// What an edit box shows text for: a page paragraph, or text added in this session.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum TextTarget {
    Block(usize),
    Added(usize),
}

/// How many faint Edit-text outlines are drawn per page per frame. CAD plots can draw
/// ~1000 tiny images on one page; stroking all of them every frame (plus a hover query
/// per box) stalls the UI. Past this, only the selected and hovered boxes are outlined:
/// hit-testing still covers every image, so nothing becomes unselectable.
pub(crate) const MAX_OUTLINES: usize = 250;

/// The char offset of byte `at` in `text`; the end when `at` is past it or inside a char.
fn char_of(text: &str, at: usize) -> usize {
    text.get(..at).map_or_else(|| text.chars().count(), |head| head.chars().count())
}

/// The byte offset of char `at` in `text`; the end when `at` is past it.
fn byte_of(text: &str, at: usize) -> usize {
    text.char_indices().nth(at).map_or(text.len(), |(byte, _)| byte)
}

/// The bytes of `text` that `selection` selects: the editor's selection is in chars, and the engine
/// (formatting ranges, source runs) addresses bytes.
pub(crate) fn selected_bytes(text: &str, selection: &TextSelection) -> Range<usize> {
    let chars = selection.range();
    byte_of(text, chars.start)..byte_of(text, chars.end)
}

/// Format text ▸ Superscript or Subscript on `text`: on the `selection`, or all of it without one
/// ([`pdfcraft_engine::text_script::toggle`]). A selection follows the converted characters and keeps
/// which end is the caret. Whether the text changed.
pub(crate) fn toggle_script(text: &mut String, selection: &mut Option<TextSelection>, script: pdfcraft_engine::text_script::Script) -> bool {
    if selection.is_some_and(|s| s.is_empty()) {
        *selection = None;
    }
    let bytes = selection.as_ref().map(|s| selected_bytes(text, s));
    let Some((toggled, range)) = pdfcraft_engine::text_script::toggle(text, bytes.as_ref(), script) else { return false };
    if let Some(old) = *selection {
        let (start, end) = (char_of(&toggled, range.start), char_of(&toggled, range.end));
        *selection = Some(if old.caret >= old.anchor { TextSelection::new(start, end) } else { TextSelection::new(end, start) });
    }
    *text = toggled;
    true
}

/// What a paragraph box's text, selection and rows' alignments were at one point of its typing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Typed {
    /// The paragraph's text.
    pub text: String,
    /// The selection, in chars of `text`.
    pub selection: Option<TextSelection>,
    /// The rows' own alignments, one for each line of `text`.
    pub aligns: Vec<Option<pdfcraft_engine::TextAlign>>,
}

impl Typed {
    /// `text` with `selection` and the rows' `aligns`: the one way a box builds its value.
    pub(crate) fn of(text: String, selection: Option<TextSelection>, aligns: Vec<Option<pdfcraft_engine::TextAlign>>) -> Self {
        Typed { text, selection, aligns }
    }
}

/// A run of typing or deleting that is one undo step so far.
#[derive(Clone, Debug)]
struct OpenRun {
    kind: EditKind,
    id: GestureId,
    from: Typed,
}

/// As many texts as the box's alignment memory keeps, as many as its history does.
const ALIGN_MEMORY: usize = 100;

/// One open text box's Undo and Redo, over [`EditHistory`]. A run of typing, or of deleting, is one
/// step; a paste or a composition's commit is one step by itself. The rows' alignments by text are
/// kept beside it, since a step puts text back and its alignments must come with it. The history
/// lives as long as the box's session and starts from the text the page has: nothing of another
/// paragraph, nor of the text before a format, can come back.
#[derive(Clone, Debug)]
pub(crate) struct TypingHistory {
    history: EditHistory<Typed>,
    /// The box as of its last record, the `before` of the next one.
    current: Typed,
    /// The text before the first provisional IME composition update.
    composition_before: Option<Typed>,
    run: Option<OpenRun>,
    next_gesture: GestureId,
    /// Each text the box has shown through its own typing, with its rows' alignments then, oldest
    /// first. A picked alignment makes them stale ([`Self::pick_align`]).
    seen: Vec<(String, Vec<Option<pdfcraft_engine::TextAlign>>)>,
}

impl TypingHistory {
    /// A history that starts at `text`.
    pub(crate) fn new(text: Typed) -> Self {
        TypingHistory {
            history: EditHistory::new(&text),
            current: text,
            composition_before: None,
            run: None,
            next_gesture: GestureId::new(0),
            seen: Vec::new(),
        }
    }

    /// The box went to `now` through an edit of `kind`. Its aligns are already followed to the new
    /// text ([`pdfcraft_engine::paragraph_align::remap_at`]).
    pub(crate) fn record(&mut self, kind: EditKind, now: Typed) {
        let before = std::mem::replace(&mut self.current, now.clone());
        self.remember(&before.text, &before.aligns);
        self.remember(&now.text, &now.aligns);
        match kind {
            EditKind::Typed | EditKind::Deleted => {
                self.composition_before = None;
                if self.run.as_ref().is_none_or(|run| run.kind != kind) {
                    self.end_run(&before);
                    let id = self.next_gesture;
                    self.next_gesture = id.next();
                    // The step's name is never shown: a box's history has no menu.
                    self.history.record(&EditEvent::begin(id, Mixed::Same(before.clone()), Arc::from(""), None));
                    self.run = Some(OpenRun { kind, id, from: before });
                }
            }
            // The marked text of a composition is in the box but no step: its commit is the one.
            EditKind::Composing => {
                self.end_run(&before);
                self.composition_before.get_or_insert(before);
            }
            _ => {
                self.end_run(&before);
                let before = if kind == EditKind::Committed {
                    self.composition_before.take().unwrap_or(before)
                } else {
                    self.composition_before = None;
                    before
                };
                self.history.record(&EditEvent::set(Mixed::Same(before), now, Arc::from(""), None));
            }
        }
    }

    /// Steps `now` back or forward. False when there is nowhere to go. The rows' alignments follow:
    /// a text the box showed gets back the alignments it had then, so Undo bringing back a deleted row
    /// brings back its alignment; a new text follows the edit.
    pub(crate) fn step(&mut self, now: &mut Typed, step: Step) -> bool {
        let last = self.current.clone();
        self.end_run(&last);
        let before = now.clone();
        if !self.history.step(now, step) {
            return false;
        }
        self.follow_rows(&before, now);
        self.current = now.clone();
        self.composition_before = None;
        true
    }

    /// An alignment was picked: the alignments of the texts the box showed are stale, and a step
    /// follows the edit on the current ones instead, so Undo never takes back the pick.
    pub(crate) fn pick_align(&mut self) {
        self.seen.clear();
    }

    /// A format was applied: the page has `now`, and the box starts again from it, so a Redo cannot put
    /// back typing that was undone before the format.
    pub(crate) fn restart(&mut self, now: Typed) {
        self.history = EditHistory::new(&now);
        self.current = now;
        self.composition_before = None;
        self.run = None;
        self.seen.clear();
    }

    /// Whether an Undo would change the box.
    pub(crate) fn can_undo(&self) -> bool {
        self.run.is_some() || self.history.can_undo()
    }

    /// Whether a Redo would change the box.
    pub(crate) fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Closes the open run as one step ending at `last`.
    fn end_run(&mut self, last: &Typed) {
        if let Some(run) = self.run.take() {
            self.history.record(&EditEvent::commit(run.id, Mixed::Same(run.from), last.clone()));
        }
    }

    fn remember(&mut self, text: &str, aligns: &[Option<pdfcraft_engine::TextAlign>]) {
        self.seen.retain(|(seen, _)| seen != text);
        self.seen.push((text.to_owned(), aligns.to_vec()));
        if self.seen.len() > ALIGN_MEMORY {
            self.seen.remove(0);
        }
    }

    /// `now` is what a step left of the box that was `before`: its aligns come from the memory of its
    /// text, else follow the edit with the caret saying which of two same rows changed.
    fn follow_rows(&mut self, before: &Typed, now: &mut Typed) {
        self.remember(&before.text, &before.aligns);
        let caret = now.selection.map(|s| byte_of(&now.text, s.caret));
        let restored = self.seen.iter().rev().find(|(text, _)| *text == now.text);
        now.aligns = match restored {
            Some((_, was)) => was.clone(),
            None => pdfcraft_engine::paragraph_align::remap_at(&before.text, &now.text, &before.aligns, caret),
        };
        let aligns = now.aligns.clone();
        self.remember(&now.text, &aligns);
    }
}

/// A paragraph being edited.
#[derive(Clone, Debug)]
pub struct LineEditor {
    /// The session's selection in chars of `text`; an empty one is the caret.
    pub selection: Option<TextSelection>,
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
    /// The draft extends outside the source container; its frame follows the live draft bounds.
    pub overflow: bool,
    /// The initial source container width in screen pixels.
    max_width: f32,
    /// Current screen-space size. Recomputed from the current page transform before painting.
    size: f32,
    /// Take the keyboard focus on the next frame.
    pub focus: bool,
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
    /// The box's own Undo and Redo, which starts from the text the page has.
    pub(crate) history: TypingHistory,
}

/// Two editors are equal when they hold the same paragraph in the same state: the history is a
/// record of how it got there, not part of what it is.
impl PartialEq for LineEditor {
    fn eq(&self, other: &Self) -> bool {
        self.selection == other.selection
            && self.font_resource == other.font_resource
            && self.page == other.page
            && self.block == other.block
            && self.text == other.text
            && self.original == other.original
            && self.rect == other.rect
            && self.source_rect == other.source_rect
            && self.multiline == other.multiline
            && self.source == other.source
            && self.overflow == other.overflow
            && self.max_width == other.max_width
            && self.size == other.size
            && self.focus == other.focus
            && self.look == other.look
            && self.look0 == other.look0
            && self.family_chosen == other.family_chosen
            && self.size_chosen == other.size_chosen
            && self.extras == other.extras
            && self.extras0 == other.extras0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extras {
    pub underline: bool,
    pub line_spacing: f64,
    pub char_spacing: f64,
    pub scale: f64,
}

impl Default for Extras {
    fn default() -> Self {
        Extras { underline: false, line_spacing: 0.0, char_spacing: 0.0, scale: 100.0 }
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

    /// The box as its history sees it.
    pub(crate) fn typed(&self) -> Typed {
        Typed::of(self.text.clone(), self.selection, self.look.aligns.clone())
    }

    /// The box set to `typed`, as a step of its history leaves it. The alignments the paragraph was
    /// opened with follow the new text, as they do for typing.
    pub(crate) fn set_typed(&mut self, typed: Typed) {
        let before = std::mem::replace(&mut self.text, typed.text);
        self.look0.aligns = pdfcraft_engine::paragraph_align::remap(&before, &self.text, &self.look0.aligns);
        self.look.aligns = typed.aligns;
        self.selection = typed.selection;
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
        if let Some(selection) = self.selection.filter(|s| !s.is_empty())
            && (style.family.is_some()
                || style.resource.is_some()
                || style.size.is_some()
                || style.color.is_some()
                || style.underline.is_some()
                || style.char_spacing.is_some()
                || style.scale.is_some())
        {
            style.ranges.push(pdfcraft_engine::RangeStyle {
                range: selected_bytes(&self.text, &selection),
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

    /// After applying formatting: the paragraph now has it. Its old selection is cleared; the
    /// rewritten paragraph may have different text or geometry.
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
        self.history.restart(self.typed());
        self.focus = true;
    }

    /// Adopt the rewritten paragraph's current geometry before the next overlay frame.
    pub(crate) fn refresh_source(&mut self, block: &pdfcraft_engine::TextBlock, lines: Vec<pdfcraft_engine::TextLine>) {
        self.source_rect = block.rect.map(|v| v as f32);
        self.multiline = block.rows.len() > 1;
        self.source = Some((block.clone(), lines));
        let before = std::mem::replace(&mut self.text, block.editor_text.clone());
        if self.text != before {
            // Char offsets of the old text: they may name other characters, or none, in the new.
            self.selection = None;
        }
        for look in [&mut self.look, &mut self.look0] {
            look.aligns = pdfcraft_engine::paragraph_align::remap(&before, &self.text, &look.aligns);
        }
        self.original = self.text.clone();
        self.history.restart(self.typed());
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
        let matching_lines: Vec<&pdfcraft_engine::TextLine> = match self.selection.filter(|s| !s.is_empty()) {
            Some(selection) => {
                let range = selected_bytes(&self.text, &selection);
                spans.iter().filter(|(span, _)| span.start < range.end && span.end > range.start).filter_map(|(_, idx)| lines.get(*idx)).collect()
            }
            None => spans.iter().filter_map(|(_, idx)| lines.get(*idx)).collect(),
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

/// How far a closed paragraph's edge grabs either side of its line (screen pixels), along its whole
/// height: wide enough to catch without aiming for the hairline (#2203). The width frame's edge band
/// (`BoxRules::with_edge_band`) and the paragraph part's reach are both this wide, so the hover that
/// shows the frame covers the band its press lands on.
const EDGE_REACH: f32 = 8.0;
/// How far outside an open paragraph's frame the pointer still means the frame (screen pixels):
/// the frame's Move cursor shows, and a press there takes hold of the box (`object_ui`).
pub(crate) const FRAME_REACH: f32 = 8.0;

/// A paragraph's box on screen: its text rect (user space), with a little room around it.
fn paragraph_box(xf: &PageXform, info: &DocInfo, page: usize, rect: [f64; 4]) -> Rect {
    xf.user_rect(info, page, rect.map(|v| v as f32)).expand(2.0)
}

/// The width handles need the page upright (or upside down): on a quarter-turned page the
/// screen's horizontal is the paragraph's vertical.
fn has_edges(info: &DocInfo, page: usize) -> bool {
    info.pages.get(page).is_some_and(|p| p.rotation % 180 == 0)
}

pub(crate) fn user_box(xf: &PageXform, info: &DocInfo, page: usize, r: Rect) -> [f64; 4] {
    let p = &info.pages[page];
    let (a, b) = (xf.screen_to_view(r.min), xf.screen_to_view(r.max));
    let (u, v) = (p.view_to_user(a.0, a.1), p.view_to_user(b.0, b.1));
    [u[0].min(v[0]) as f64, u[1].min(v[1]) as f64, u[0].max(v[0]) as f64, u[1].max(v[1]) as f64]
}

/// The paragraphs of `page` as the document has them now.
fn blocks_of(session: &PageSession, doc: &pdfcraft_engine::Document, page: usize) -> Vec<pdfcraft_engine::TextBlock> {
    match session.edit_lines.get(&page) {
        Some((generation, blocks)) if *generation == doc.edit_generation() => blocks.clone(),
        _ => doc.text_blocks(page),
    }
}

/// [`blocks_of`], kept for the next call while the document is unchanged.
fn page_blocks(session: &mut PageSession, doc: &pdfcraft_engine::Document, page: usize) -> Vec<pdfcraft_engine::TextBlock> {
    let blocks = blocks_of(session, doc, page);
    session.edit_lines.insert(page, (doc.edit_generation(), blocks.clone()));
    blocks
}

/// Where a press at the closed paragraph `i` of `boxes` reaches: its box and, on an upright page, the
/// reach of its edges ([`EDGE_REACH`]). Where that reach runs toward a box above or below, it ends
/// halfway between them: of two boxes the pointer is level with the nearer one, and the arena's own
/// tie rule (the nearest centre) would not say that.
fn reach_of(boxes: &[Rect], i: usize, upright: bool) -> Rect {
    let Some(b) = boxes.get(i).copied() else { return Rect::NOTHING };
    if !upright {
        return b;
    }
    let mut reach = b.expand(EDGE_REACH);
    for (j, other) in boxes.iter().enumerate() {
        if j == i || other.right() + EDGE_REACH < reach.left() || other.left() - EDGE_REACH > reach.right() {
            continue;
        }
        if other.top() >= b.bottom() {
            reach.max.y = reach.max.y.min((b.bottom() + other.top()) / 2.0);
        } else if other.bottom() <= b.top() {
            reach.min.y = reach.min.y.max((other.bottom() + b.top()) / 2.0);
        } else {
            // An overlapping box owns points inside its height ahead of this box's extended reach.
            if other.top() < b.top() {
                reach.min.y = reach.min.y.max(b.top());
            }
            if other.bottom() > b.bottom() {
                reach.max.y = reach.max.y.min(b.bottom());
            }
        }
    }
    reach
}

/// The parts of Edit text's closed paragraphs on `page`: one for each, over its box and its edges'
/// reach, taking a click (opens it) and a drag (moves it). The open paragraph has none: its editor
/// takes the pointer over its own rect.
// PLAN: A-4 gives `page_parts(view, page, placed)`, but a paragraph's box needs the page's crop and
// turn (`doc.info`), which the session has no way to reach; `doc` is a parameter, as in A-5's
// `picture_parts` (pdf-canvas-w9 passes it).
pub(crate) fn page_parts(
    session: &PageSession,
    doc: &pdfcraft_engine::Document,
    page: usize,
    placed: &PlacedPage,
    geo: PageGeometry<'_>,
    adding_text: bool,
) -> Vec<PagePart> {
    let Some(xf) = session.page_xform(page, geo) else { return Vec::new() };
    let info = &doc.info;
    let open = session.line_editor.as_ref().filter(|e| e.page == page).map(|e| e.block);
    let boxes: Vec<Rect> = blocks_of(session, doc, page).iter().map(|l| paragraph_box(&xf, info, page, l.rect)).collect();
    let upright = has_edges(info, page);
    let cursor = if open.is_some() { CursorIcon::Text } else { CursorIcon::Move };
    boxes
        .iter()
        .enumerate()
        .filter(|(block, _)| Some(*block) != open)
        .filter_map(|(block, &bounds)| {
            let reach = if adding_text { bounds } else { reach_of(&boxes, block, upright) }.intersect(placed.rect);
            if !reach.is_positive() {
                return None;
            }
            let shape = if adding_text || !upright {
                HitShape::Rect(reach)
            } else {
                let inner = (bounds.width() / 4.0).min(EDGE_REACH);
                let left = (bounds.left() + inner).clamp(reach.left(), reach.right());
                let right = (bounds.right() - inner).clamp(reach.left(), reach.right());
                let top = bounds.top().clamp(reach.top(), reach.bottom());
                let bottom = bounds.bottom().clamp(reach.top(), reach.bottom());
                HitShape::Polygon(vec![
                    egui::pos2(reach.left(), reach.top()),
                    egui::pos2(left, reach.top()),
                    egui::pos2(left, top),
                    egui::pos2(right, top),
                    egui::pos2(right, reach.top()),
                    egui::pos2(reach.right(), reach.top()),
                    egui::pos2(reach.right(), reach.bottom()),
                    egui::pos2(right, reach.bottom()),
                    egui::pos2(right, bottom),
                    egui::pos2(left, bottom),
                    egui::pos2(left, reach.bottom()),
                    egui::pos2(reach.left(), reach.bottom()),
                ])
            };
            Some(PagePart::new(Owner::Paragraph { block }, page, shape, PageLayer::Text).cursor(cursor))
        })
        .collect()
}

/// Whether a drag may move a paragraph on `page`: not while a paragraph there is open for typing, nor
/// while text that can't be drawn where it is being typed (#125) keeps the box where it is.
fn movable(session: &PageSession, page: usize) -> bool {
    session.line_editor.as_ref().is_none_or(|e| e.page != page) && session.content.blocked().is_none()
}

/// A press on a paragraph's part. A click opens the paragraph (or, when another has changes, applies
/// that one and opens this next frame); a drag of the body moves it on release, and nothing moves
/// when the pointer is released where it began. The drag is the press's own phases: nothing is held
/// between passes, and [`paragraph_controls`] draws the preview from the same press.
pub(crate) fn paragraph_press(session: &mut PageSession, doc: &pdfcraft_engine::Document, press: &PagePress, geo: PageGeometry<'_>) -> Option<Edit> {
    let Owner::Paragraph { block } = press.owner else { return None };
    let page = press.page;
    if session.objects.input_blocked() {
        return None;
    }
    let PressPhase::Release { start, pos, click, .. } = &press.phase else { return None };
    if *click {
        return open_by_click(session, doc, page, block, geo);
    }
    if !movable(session, page) {
        return None;
    }
    let delta = *pos - start.origin;
    // Translation must keep the selected source operators. Rewriting unchanged text
    // would also remove unrelated paragraphs that now overlap this one (#594).
    let xf = session.page_xform(page, geo)?;
    doc.info.pages.get(page)?;
    let blocks = blocks_of(session, doc, page);
    let b = paragraph_box(&xf, &doc.info, page, blocks.get(block)?.rect);
    // A paragraph index belongs to the document where the press began: after an edit it may name
    // another paragraph, which the press did not start on.
    if !b.expand(EDGE_REACH).contains(start.origin) {
        return None;
    }
    let (from, to) = (user_box(&xf, &doc.info, page, b), user_box(&xf, &doc.info, page, b.translate(delta)));
    (delta != Vec2::ZERO).then(|| Edit::MoveObjects {
        page,
        objects: vec![ObjectTarget { kind: ObjectKind::Text, index: block }],
        offset: [to[0] - from[0], to[1] - from[1]],
    })
}

/// A click on paragraph `block` of `page`.
fn open_by_click(session: &mut PageSession, doc: &pdfcraft_engine::Document, page: usize, block: usize, geo: PageGeometry<'_>) -> Option<Edit> {
    let blocks = page_blocks(session, doc, page);
    session.image_selection = None;
    // Text being typed is finished first; text that can't be drawn stays where it is being typed (#125).
    let mut finished = None;
    if session.content.draft.is_some() {
        if session.content.hold_blocked() {
            return None;
        }
        finished = session.content.finish_draft(doc.added());
        session.content.selected = None;
    }
    // A click that moves the editor to another paragraph applies the open one first, so its typed
    // text is not dropped with the switched-away editor (#2022). A click on the open box's own frame
    // keeps it open instead.
    if session.line_editor.as_ref().is_some_and(|e| e.has_changes()) {
        if session.line_editor.as_ref().is_some_and(|open| open.page == page && open.block == block) {
            return finished;
        }
        let applied = session.line_editor.take().and_then(|old| {
            let style = old.style();
            (old.text != old.original || style != pdfcraft_engine::BlockStyle::default()).then(|| {
                session.applied_editor = Some(old.clone());
                Edit::EditTextBlock { page: old.page, block: old.block, text: old.text, style }
            })
        });
        // The clicked box opens next frame, from the document with that edit applied.
        session.pending_text_open = blocks.get(block).map(|l| (page, pdfcraft_engine::BlockKey::of(l)));
        return applied.or(finished);
    }
    open_editor(session, doc, page, block, geo);
    finished
}

/// Open `block` on `page` in the inline editor. The box's geometry and source are read from the
/// document's current inventory.
fn open_editor(session: &mut PageSession, doc: &pdfcraft_engine::Document, page: usize, block: usize, geo: PageGeometry<'_>) {
    session.pending_text_open = None;
    let blocks = page_blocks(session, doc, page);
    let (Some(l), Some(xf)) = (blocks.get(block), session.page_xform(page, geo)) else { return };
    if doc.info.pages.get(page).is_none() {
        return;
    }
    let b = paragraph_box(&xf, &doc.info, page, l.rect);
    session.image_selection = None;
    let source_lines = doc.text_lines(page);
    let text = l.editor_text.clone();
    // Screen pixels per point, from the box's width.
    let scale = (b.width() - 4.0) / ((l.rect[2] - l.rect[0]).max(1.0) as f32);
    // How far the editor box may grow: a single-line paragraph's rewrite grows to the
    // page's right edge, a multi-line paragraph rewraps to its own width.
    let right = xf.rect.right().min(geo.viewport.right()) - 6.0;
    let max_width = if l.rows.len() == 1 { (right - b.left()).max(b.width()) } else { b.width() };
    let look = look_of(l);
    session.line_editor = Some(LineEditor {
        selection: None,
        font_resource: None,
        page,
        block,
        history: TypingHistory::new(Typed::of(text.clone(), None, look.aligns.clone())),
        text: text.clone(),
        original: text,
        rect: b,
        source_rect: l.rect.map(|v| v as f32),
        multiline: l.rows.len() > 1,
        source: Some((l.clone(), source_lines)),
        overflow: false,
        max_width,
        size: (l.size as f32 * scale).clamp(8.0, 72.0),
        focus: true,
        look: look.clone(),
        look0: look,
        family_chosen: false,
        size_chosen: false,
        extras: Extras::default(),
        extras0: Extras::default(),
    });
}

/// A paragraph box's rect in document units.
fn doc_rect(view: &CanvasView, screen: Rect) -> DocRect {
    DocRect::from_points(view.to_doc(screen.min), view.to_doc(screen.max))
}

/// A rect in document units as a path, for the outline controls.
fn rect_path(rect: DocRect) -> BezPath {
    rect.to_path(0.1)
}

/// Edit text's paragraph controls on the pages of `body`: the faint outline of every closed
/// paragraph, the box being dragged, the width frame of the paragraph under the pointer, and the open
/// paragraph's editor. The press and the hover are the pass's part output's (`parts`): the press when
/// its owner is a paragraph, and the `(page, block)` of the paragraph under the pointer. Returns the
/// edit a rewrap or an applied paragraph gives.
// A paragraph's width-frame handles count as that paragraph for the hover, so the frame stays shown
// while the pointer crosses onto a handle and through its drag.
pub(crate) fn paragraph_controls(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &mut PageSession,
    doc: &pdfcraft_engine::Document,
    parts: &PagePartsOutput,
    geo: PageGeometry<'_>,
    adding_text: bool,
) -> Option<Edit> {
    let press = parts.press.as_ref().filter(|p| matches!(p.owner, Owner::Paragraph { .. }));
    let hovered = match parts.hovered {
        Some((Owner::Paragraph { block } | Owner::Frame { block, .. }, page)) => Some((page, block)),
        _ => None,
    };
    let mut edit = None;
    for placed in body.pages {
        let page = placed.page;
        let xf = geo.xform_of(placed);
        let blocks = page_blocks(session, doc, page);
        let boxes: Vec<Rect> = blocks.iter().map(|l| paragraph_box(&xf, &doc.info, page, l.rect)).collect();
        open_waiting(session, doc, page, &blocks, &boxes, geo);
        outline_closed(ui, body, session, page, &boxes, xf.rect.intersect(geo.viewport));
        draw_move(ui, body, session, page, &boxes, press);
        let held = blocks
            .iter()
            .enumerate()
            .find_map(|(block, _)| craft::ui::runtime::pointer::active_part(ui.ctx(), width_frame_id(ui, page, block)).map(|_| (page, block)));
        let framed = held
            .or(hovered)
            .filter(|(p, _)| *p == page)
            .and_then(|(_, block)| blocks.get(block).zip(boxes.get(block).copied()).map(|(l, b)| (block, l, b)));
        let mut clicked = None;
        edit = edit.or_else(|| width_frame(ui, body, session, doc, page, framed, geo, &mut clicked));
        if adding_text && let Some(block) = clicked {
            edit = edit.or_else(|| open_by_click(session, doc, page, block, geo));
        }
    }
    edit.or_else(|| editor_session(ui, body, session, doc, geo))
}

fn width_frame_id(ui: &egui::Ui, page: usize, block: usize) -> egui::Id {
    ui.make_persistent_id(("paragraph-width", page, block))
}

/// Opens the paragraph a switching click, or a click on grouped text, asked for once the document is
/// ready for it.
fn open_waiting(
    session: &mut PageSession,
    doc: &pdfcraft_engine::Document,
    page: usize,
    blocks: &[pdfcraft_engine::TextBlock],
    boxes: &[Rect],
    geo: PageGeometry<'_>,
) {
    let open_here = session.line_editor.as_ref().is_some_and(|e| e.page == page);
    let gate = !session.objects.input_blocked();
    // A box a switching click asked for opens now, once the edit queued with that click has
    // applied and `blocks`/`boxes` are the document's new inventory (#2022).
    if gate
        && !open_here
        && let Some((open_page, key)) = session.pending_text_open.clone()
        && open_page == page
    {
        // The edit may have regrouped the page: open the paragraph that was clicked, wherever it
        // is now, and none if it was merged into another (#2516).
        match key.find(blocks, 0) {
            Some(block) => open_editor(session, doc, page, block, geo),
            None => session.pending_text_open = None,
        }
    }
    // A click on grouped text prepared the page: the paragraph now at the point clicked opens, and
    // none if the point is on the group's artwork. A preparation that failed left the page as it
    // was, where no paragraph is at that point. It is taken either way, once the queued
    // preparation has run; this control also runs in the pass that queued it.
    let open_here = session.line_editor.as_ref().is_some_and(|e| e.page == page);
    if session.pending_edit.is_none()
        && let Some((_, [x, y])) = session.grouped_text_click.take_if(|(p, _)| *p == page)
        && gate
        && !open_here
        && let Some(xf) = session.page_xform(page, geo)
        && let at = user_to_screen(&xf, &doc.info, page, x, y)
        && let Some(block) = boxes.iter().rposition(|b| b.contains(at))
    {
        open_editor(session, doc, page, block, geo);
    }
}

/// The faint outline of every closed paragraph on `page`, one control with every path.
fn outline_closed(ui: &mut egui::Ui, body: &PageBody<'_>, session: &PageSession, page: usize, boxes: &[Rect], clip: Rect) {
    let open = session.line_editor.as_ref().filter(|e| e.page == page).map(|e| e.block);
    let paths: Vec<BezPath> =
        boxes.iter().enumerate().filter(|(i, _)| Some(*i) != open).take(MAX_OUTLINES).map(|(_, b)| rect_path(doc_rect(&body.view, *b))).collect();
    if !paths.is_empty() {
        let _ = SelectionOutline::faint(tl!("Edit text & images").to_string(), &paths).color(crate::object_ui::accent_ink(ui, 0.9)).show_at(ui, clip);
    }
}

/// The box of the paragraph being dragged, where the pass's press has taken it.
fn draw_move(ui: &mut egui::Ui, body: &PageBody<'_>, session: &PageSession, page: usize, boxes: &[Rect], press: Option<&PagePress>) {
    let Some(press) = press.filter(|p| p.page == page) else { return };
    let Owner::Paragraph { block } = press.owner else { return };
    let (PressPhase::DragStart { start, pos, .. } | PressPhase::Drag { start, pos, .. }) = &press.phase else { return };
    if !movable(session, page) {
        return;
    }
    let Some(b) = boxes.get(block) else { return };
    // The press began on this paragraph's box, as at its release.
    if !b.expand(EDGE_REACH).contains(start.origin) {
        return;
    }
    let path = [rect_path(doc_rect(&body.view, b.translate(*pos - start.origin)))];
    let _ = SelectionOutline::solid(tl!("Edit text & images").to_string(), &path).show_at(ui, body.rect);
}

/// The width frame of `framed`, the paragraph under the pointer and its box: the left and right edge
/// handles, no corners. The box keeps its own drag between passes, so nothing is held here. Its
/// release rewraps the paragraph to the new width, moving it with its left side. Not while a
/// paragraph is open on the page, nor while the objects selected together or added content being moved
/// hold the page.
fn width_frame(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &PageSession,
    doc: &pdfcraft_engine::Document,
    page: usize,
    framed: Option<(usize, &pdfcraft_engine::TextBlock, Rect)>,
    geo: PageGeometry<'_>,
    clicked: &mut Option<usize>,
) -> Option<Edit> {
    let info = &doc.info;
    let xf = session.page_xform(page, geo)?;
    let open_here = session.line_editor.as_ref().is_some_and(|e| e.page == page);
    if open_here || !has_edges(info, page) || session.objects.holds(page) {
        return None;
    }
    let (block, l, b) = framed?;
    let start = doc_rect(&body.view, b);
    let state = BoxState::from_rect(start);
    // `TransformBox::text_frame`'s rules, showing the left and right handles only, each side grabbed
    // anywhere along the paragraph's height.
    let rules = BoxRules::default()
        .with_scale(ScaleRule::Free)
        .with_aspect_linked(false)
        .with_allow(BoxAllow::new(false, false, false).with_warp(false))
        .with_min_side(4.0)
        .with_handles(BoxHandles::Sides)
        .with_edge_band(EDGE_REACH);
    let output = TransformBox::text_frame(tl!("Width").to_string(), Bind::given(state), start)
        .id(width_frame_id(ui, page, block))
        .rules(rules)
        .show_at(ui, body.rect);
    if output.clicked.is_some_and(|(_, point)| b.contains(body.view.to_screen(point))) {
        *clicked = Some(block);
    }
    let after = output.edit.events.iter().find_map(|event| match event {
        EditEvent::Commit { after, .. } | EditEvent::Set { after, .. } => Some(*after),
        _ => None,
    })?;
    let preview = Rect::from_two_pos(body.view.to_screen(after.quad[0]), body.view.to_screen(after.quad[2]));
    // A click on a handle with no drag leaves the box where it was.
    if preview == b || info.pages.get(page).is_none() {
        return None;
    }
    let (from, to) = (user_box(&xf, info, page, b), user_box(&xf, info, page, preview));
    // The new width in user space: the box's change, added to the paragraph's own. A moved left side
    // (in user space; on an upside-down page that's the screen's right edge) moves the paragraph with it.
    let width = (l.rect[2] - l.rect[0]) + (to[2] - to[0]) - (from[2] - from[0]);
    let shift = to[0] - from[0];
    let style = pdfcraft_engine::BlockStyle { width: Some(width), offset: (shift != 0.0).then_some([shift, 0.0]), ..Default::default() };
    // `editor_text` has a newline for every visual row, so a wrap would come back as a hard break and
    // a wider box would not reflow it. `reflow_text` rejoins a wrap and keeps a newline that was typed.
    Some(Edit::EditTextBlock { page, block, text: l.reflow_text.clone(), style })
}

/// The open paragraph's text for [`TextEditor`]: the editor's text, laid out as the page lays it out
/// (source runs, faces, OCR size) at the page's scale, mapped to document units through the canvas
/// view. Every edit goes into the box's own history.
struct ParagraphHost<'a> {
    ctx: &'a egui::Context,
    ed: &'a mut LineEditor,
    faces: &'a BTreeMap<String, FontRef>,
    view: CanvasView,
    /// Screen pixels per page point.
    scale: f32,
    /// The layout width, in screen pixels.
    width: f32,
    /// Where the laid-out text's top left is on screen.
    origin: Pos2,
    galley: Option<Arc<egui::Galley>>,
}

/// The chars each row of `galley` holds, without its line break.
fn row_ranges(galley: &egui::Galley) -> Vec<Range<usize>> {
    let mut at = 0;
    galley
        .rows
        .iter()
        .map(|placed| {
            let range = at..at + placed.row.glyphs.len();
            at = range.end + usize::from(placed.ends_with_newline);
            range
        })
        .collect()
}

/// Where column `column` of a row starts, from its glyphs' `(x, advance)`: the end of the row past its last.
fn x_at(columns: &[(f32, f32)], column: usize) -> f32 {
    columns.get(column).map(|c| c.0).or_else(|| columns.last().map(|c| c.0 + c.1)).unwrap_or(0.0)
}

fn doc_pos(p: Point) -> Pos2 {
    pos2(p.x as f32, p.y as f32)
}

/// The galley cursor at char `offset`.
fn cursor_at(offset: usize) -> egui::text::CCursor {
    egui::text::CCursor::new(CharIndex(offset))
}

impl ParagraphHost<'_> {
    /// The laid-out text, laid out again once an edit has changed it.
    fn galley(&mut self) -> Arc<egui::Galley> {
        let (ctx, ed, faces, scale, width) = (self.ctx, &*self.ed, self.faces, self.scale, self.width);
        Arc::clone(self.galley.get_or_insert_with(|| editor_galley(ctx, ed, faces, &ed.text, scale, width)))
    }

    /// A point of the laid-out text (screen pixels from its top left) in document units.
    fn to_doc(&self, p: Vec2) -> Point {
        self.view.to_doc(self.origin + p)
    }

    /// A point in document units as the laid-out text's screen pixels from its top left.
    fn from_doc(&self, p: Point) -> Vec2 {
        self.view.to_screen(p) - self.origin
    }

    /// The cursor at char `offset` on laid-out line `line`: at a soft wrap the offset ends one line
    /// and starts the next.
    fn cursor(&mut self, offset: usize, line: Option<usize>) -> egui::text::CCursor {
        let galley = self.galley();
        let mut cursor = cursor_at(offset);
        if let Some(line) = line
            && galley.layout_from_cursor(cursor).row != line
        {
            cursor.prefer_next_row = true;
        }
        cursor
    }

    /// The screen-pixel rows `range` covers, from the galley's top left, as `(left, right, top, bottom)`.
    fn pieces(&mut self, range: Range<usize>) -> Vec<[f32; 4]> {
        let galley = self.galley();
        let mut out = Vec::new();
        for (placed, chars) in galley.rows.iter().zip(row_ranges(&galley)) {
            let (start, end) = (range.start.max(chars.start), range.end.min(chars.end));
            if start >= end {
                continue;
            }
            let columns: Vec<(f32, f32)> = placed.row.glyphs.iter().map(|g| (g.pos.x, g.advance_width)).collect();
            let (left, right) = (x_at(&columns, start - chars.start), x_at(&columns, end - chars.start));
            out.push([placed.pos.x + left, placed.pos.x + right, placed.pos.y, placed.pos.y + placed.row.size.y]);
        }
        out
    }
}

impl TextEditorHost for ParagraphHost<'_> {
    fn text(&self) -> &str {
        &self.ed.text
    }

    fn replace(&mut self, range: Range<usize>, text: &str, kind: EditKind) -> Option<usize> {
        let before = self.ed.text.clone();
        let bytes = byte_of(&before, range.start)..byte_of(&before, range.end);
        self.ed.text.replace_range(bytes, text);
        let caret = range.start + text.chars().count();
        // Rows aligned on their own keep their alignment; the caret, at the edit's end, says which of
        // two same rows changed.
        let caret_byte = byte_of(&self.ed.text, caret);
        for look in [&mut self.ed.look, &mut self.ed.look0] {
            look.aligns = pdfcraft_engine::paragraph_align::remap_at(&before, &self.ed.text, &look.aligns, Some(caret_byte));
        }
        self.ed.selection = Some(TextSelection::caret_at(caret));
        let now = self.ed.typed();
        self.ed.history.record(kind, now);
        self.galley = None;
        Some(text.chars().count())
    }

    fn hit(&mut self, at: Point) -> (usize, usize) {
        let galley = self.galley();
        let cursor = galley.cursor_from_pos(self.from_doc(at));
        (cursor.index.0, galley.layout_from_cursor(cursor).row)
    }

    fn caret(&mut self, offset: usize, line: Option<usize>) -> Option<CaretLine> {
        let cursor = self.cursor(offset, line);
        let rect = self.galley().pos_from_cursor(cursor);
        Some(CaretLine::new(self.to_doc(rect.left_top().to_vec2()), self.to_doc(rect.left_bottom().to_vec2())))
    }

    fn selection_quads(&mut self, range: Range<usize>) -> Vec<[Point; 4]> {
        self.pieces(range)
            .into_iter()
            .map(|[left, right, top, bottom]| {
                [self.to_doc(vec2(left, top)), self.to_doc(vec2(right, top)), self.to_doc(vec2(right, bottom)), self.to_doc(vec2(left, bottom))]
            })
            .collect()
    }

    fn underline_segments(&mut self, range: Range<usize>) -> Vec<[Point; 2]> {
        self.pieces(range).into_iter().map(|[left, right, _, bottom]| [self.to_doc(vec2(left, bottom)), self.to_doc(vec2(right, bottom))]).collect()
    }

    fn lines(&mut self) -> Vec<HostLine> {
        let galley = self.galley();
        let text = self.ed.text.clone();
        let mut lines = Vec::new();
        for (placed, chars) in galley.rows.iter().zip(row_ranges(&galley)) {
            let line: String = text.chars().skip(chars.start).take(chars.len()).collect();
            let rect = Rect::from_two_pos(doc_pos(self.to_doc(placed.pos.to_vec2())), doc_pos(self.to_doc((placed.pos + placed.row.size).to_vec2())));
            let baseline = placed.row.glyphs.first().map_or(placed.row.size.y, |g| g.pos.y);
            let baseline = doc_pos(self.to_doc(vec2(placed.pos.x, placed.pos.y + baseline))).y;
            let positions = placed.row.glyphs.iter().map(|g| self.view.doc_len(g.pos.x) as f32).collect();
            let widths = placed.row.glyphs.iter().map(|g| self.view.doc_len(g.advance_width) as f32).collect();
            lines.push(HostLine::new(line.clone(), chars, rect, baseline, vec![HostRun::new(0..line.len(), false, positions, widths)]));
        }
        lines
    }

    fn line_range(&mut self, line: usize) -> Range<usize> {
        let galley = self.galley();
        row_ranges(&galley).get(line).cloned().unwrap_or(0..0)
    }

    fn line_step(&mut self, offset: usize, lines: i32) -> usize {
        let galley = self.galley();
        let ranges = row_ranges(&galley);
        let row = galley.layout_from_cursor(cursor_at(offset)).row;
        let target = (row as i64 + i64::from(lines)).clamp(0, ranges.len().saturating_sub(1) as i64) as usize;
        let (Some(placed), Some(range)) = (galley.rows.get(target), ranges.get(target)) else { return offset };
        let x = galley.pos_from_cursor(cursor_at(offset)).min.x;
        let at = galley.cursor_from_pos(vec2(x, placed.pos.y + placed.row.size.y / 2.0)).index.0;
        at.clamp(range.start, range.end)
    }

    fn line_edge(&mut self, offset: usize, end: bool) -> usize {
        let galley = self.galley();
        let row = galley.layout_from_cursor(cursor_at(offset)).row;
        row_ranges(&galley).get(row).map_or(offset, |range| if end { range.end } else { range.start })
    }
}

/// The open paragraph's editor: its changed text over the page without it, and a [`TextEditor`] session
/// for the selection, the caret and the keys. Returns the edit once the text is applied.
fn editor_session(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &mut PageSession,
    doc: &pdfcraft_engine::Document,
    geo: PageGeometry<'_>,
) -> Option<Edit> {
    let info = &doc.info;
    let page = session.line_editor.as_ref()?.page;
    let xf = session.page_xform(page, geo)?;
    info.pages.get(page)?;
    let (block, invisible) = {
        let ed = session.line_editor.as_ref()?;
        (ed.block, ed.source.as_ref().is_some_and(|(b, _)| b.invisible))
    };
    // An OCR word is invisible text over a scan: the textless background still shows the scanned
    // word, so the editor covers it, as the applied edit will.
    let background_ready = session.text_background.ready(page, block) && !invisible;
    // The measured extent of this paragraph's draft: a click straight from another paragraph
    // opens this one after the frame's measuring, which was the other one's.
    let draft_rect = session.text_background.draft_rect_for(page, block).map(|r| xf.user_rect(info, page, r.map(|v| v as f32)));
    // The editor draws on its page only, as the page itself does: a paragraph running past the
    // page's edge (a converter relying on the page clip) must not cover the next page or the
    // tool bars (#3061).
    let page_clip = xf.rect.intersect(geo.viewport);
    let ctx = ui.ctx().clone();
    let ed = session.line_editor.as_mut()?;
    // Reproject the source box every frame. The page may have been zoomed, scrolled or rotated
    // while the format panel was open.
    ed.rect = xf.user_rect(info, ed.page, ed.source_rect);
    ed.max_width = ed.rect.width();
    let scale = (ed.rect.width() / (ed.source_rect[2] - ed.source_rect[0]).abs().max(0.01)).max(0.01);
    ed.size = (ed.look.size as f32 * scale).clamp(1.0, 300.0);
    let text_color = color32(ed.look.color);
    // A one-row draft grows toward the page's right edge.
    let room = (xf.rect.right() - ed.rect.left() - 36.0 * scale).max(ed.rect.width());
    // The layout in the paragraph's source faces, and the cover under the editor until the textless
    // page is ready.
    let cover = ed.has_changes() && !background_ready;
    let laid = ParagraphCover::new(doc, ed, scale, room, draft_rect, cover).show_at(ui, page_clip);
    let (faces, width, live) = (laid.faces, laid.width, laid.live);
    let frame = [rect_path(doc_rect(&body.view, live.expand(2.0)))];
    let _ = SelectionOutline::solid(tl!("Text editing line").to_string(), &frame).show_at(ui, page_clip);
    ed.overflow = laid.overflow;
    // An editor entirely outside its page has no visible or interactive area.
    let editor_rect = live.intersect(page_clip);
    if !editor_rect.is_positive() {
        return None;
    }
    let len = ed.text.chars().count();
    let mut selection = ed.selection.map_or_else(|| TextSelection::caret_at(len), |s| TextSelection::new(s.anchor.min(len), s.caret.min(len)));
    let origin = ed.rect.min;
    // An editor opened in this pass takes the keys at once. The press that opened it may lie in the
    // paragraph's reach outside the editor, where the editor sees a click away: it doesn't apply it.
    let fresh = std::mem::replace(&mut ed.focus, false);
    let (output, galley) = {
        let mut host = ParagraphHost { ctx: &ctx, ed: &mut *ed, faces: &faces, view: body.view, scale, width, origin, galley: None };
        // Escape cancels and selects the paragraph's box (Acrobat); ⌘Enter and a press on the page
        // background apply.
        let editor = TextEditor::new(tl!("Text editing line").to_string(), &mut host, Bind::bound(&mut selection)).esc_commits(false);
        // The focus reveal and hit area must stay on this page, even when the paragraph extends beyond it.
        let output = if fresh { editor.request_focus(FocusRequest::Take) } else { editor }.show_at(ui, editor_rect);
        (output, host.galley())
    };
    ed.selection = Some(selection);
    if ed.has_changes() {
        ParagraphText::new(origin, Arc::clone(&galley), text_color).show_at(ui, page_clip);
    }
    let ended = output.ended.filter(|end| !(fresh && *end == EditorEnd::Commit));
    session.text_background.remember(&xf, page, block, galley, origin, text_color);
    match ended {
        None => None,
        Some(EditorEnd::Cancel) => {
            // The draft is discarded, but not the paragraph: its box is selected, as in Acrobat, so
            // Delete removes it without aiming for the frame.
            let ed = session.line_editor.take()?;
            session.objects.select_paragraph_next(ed.page, ed.block);
            None
        }
        Some(_) => {
            let ed = session.line_editor.take()?;
            session.applied_editor = Some(ed.clone());
            finished_edit(ed)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use craft::ui::primitives::textures::{Revision, TextureState};
    use egui::Color32;
    use pdfcraft_engine::TextAlign::{Center, Right};

    /// A box's text and its rows' alignments, as its history holds them.
    fn typed(text: &str, aligns: &[Option<pdfcraft_engine::TextAlign>]) -> Typed {
        Typed::of(text.into(), None, aligns.to_vec())
    }

    /// The box `from` went to `after` through a deletion or typing as the host does: the rows'
    /// alignments follow the text, and the edit is recorded.
    fn edit(history: &mut TypingHistory, now: &mut Typed, kind: EditKind, after: &str) {
        let aligns = pdfcraft_engine::paragraph_align::remap(&now.text, after, &now.aligns);
        *now = Typed::of(after.into(), None, aligns);
        history.record(kind, now.clone());
    }

    /// Two paragraphs, one above the other, close enough that the reach of their edges overlaps.
    fn two_paragraphs() -> Vec<u8> {
        let streams = [
            "/P BMC BT /F1 12 Tf 20 160 Td (Alpha beta gamma delta epsilon zeta eta theta) Tj ET EMC",
            "/P BMC BT /F2 9 Tf 20 143 Td (Iota kappa lambda mu nu xi omicron pi rho) Tj ET EMC",
        ];
        let body = streams.join("\n");
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 400 300] >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R /F2 6 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
            // A second face gives each paragraph its own appearance.
            "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>".to_string(),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// Where an edge's reach above or below its box overlaps the next paragraph's edge, the
    /// paragraph the pointer is level with takes the press, whichever is drawn on top (the play
    /// session "resize grown neighbor" at 50% zoom). Driven through the page body: a press at a
    /// side edge level with one paragraph rewraps that paragraph and no other.
    #[test]
    fn an_edge_goes_to_the_box_the_pointer_is_level_with() {
        let mut app = crate::PdfCraftApp::for_unit_tests();
        app.open_bytes(&egui::Context::default(), "two.pdf", None, two_paragraphs()).unwrap();
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            app
        });
        h.run_steps(4);
        let ctx = h.ctx.clone();
        assert!(h.state_mut().execute(&ctx, "edit.edit_text"));
        h.run_steps(2);
        let blocks = |h: &egui_kittest::Harness<'static, crate::PdfCraftApp>| {
            let s = h.state();
            s.session.get(s.views[0].id).unwrap().text_blocks(0)
        };
        let rows_of = |h: &egui_kittest::Harness<'static, crate::PdfCraftApp>, start: &str| {
            blocks(h).into_iter().find(|b| b.text.starts_with(start)).map(|b| b.rows.len())
        };
        let before = blocks(&h);
        assert_eq!(before.len(), 2, "the fixture has two paragraphs: {before:?}");
        let (upper_rows, lower_rows) = (rows_of(&h, "Alpha").unwrap(), rows_of(&h, "Iota").unwrap());
        let r = h.state().views[0].page_screen_rect(0).expect("on screen");
        let k = r.width() / 400.0;
        let screen = |x: f64, y: f64| pos2(r.left() + x as f32 * k, r.top() + (300.0 - y as f32) * k);
        let drag = |h: &mut egui_kittest::Harness<'static, crate::PdfCraftApp>, from: Pos2, to: Pos2| {
            h.hover_at(from);
            h.run_steps(1);
            h.drag_at(from);
            h.run_steps(1);
            for step in 1..=5 {
                h.hover_at(from + (to - from) * (step as f32 / 5.0));
                h.run_steps(1);
            }
            h.drop_at(to);
            h.run_steps(4);
        };
        // The upper paragraph's right edge, at the height of its bottom row, where the lower
        // paragraph's reach also arrives: it is the upper one that narrows.
        let upper = before.iter().find(|b| b.text.starts_with("Alpha")).unwrap().rect;
        let edge = screen(upper[2], upper[1] + 1.0) + vec2(2.0, 0.0);
        drag(&mut h, edge, edge - vec2((upper[2] - upper[0] - 60.0) as f32 * k, 0.0));
        assert!(rows_of(&h, "Alpha").unwrap() > upper_rows, "the paragraph the pointer was level with rewrapped");
        assert_eq!(rows_of(&h, "Iota"), Some(lower_rows), "its neighbour kept its width");
        // And the lower paragraph's, at the height of its top row, where the upper one's reach arrives.
        let lower = blocks(&h).into_iter().find(|b| b.text.starts_with("Iota")).unwrap().rect;
        let edge = screen(lower[2], lower[3] - 1.0) + vec2(2.0, 0.0);
        let after_upper = rows_of(&h, "Alpha").unwrap();
        drag(&mut h, edge, edge - vec2((lower[2] - lower[0] - 50.0) as f32 * k, 0.0));
        assert!(rows_of(&h, "Iota").unwrap() > lower_rows, "the lower paragraph rewrapped");
        assert_eq!(rows_of(&h, "Alpha"), Some(after_upper), "the upper one kept its width");
    }

    /// Formatting applied restarts the box's history: a Redo past the format can't bring back typing
    /// from before it, nor an alignment from before it (FredApps on bfc6f8b1).
    #[test]
    fn formatting_applied_restarts_the_alignments_by_text() {
        let mut now = typed("A\nB", &[None, Some(Right)]);
        let mut history = TypingHistory::new(now.clone());
        edit(&mut history, &mut now, EditKind::Deleted, "A");
        assert!(history.step(&mut now, Step::Undo));
        assert_eq!(now.aligns, [None, Some(Right)], "Undo brings the deleted row back aligned");
        // Formatting applied: the page has "A\nB", and the box starts again from it.
        history.restart(now.clone());
        let mut redone = now.clone();
        assert!(!history.step(&mut redone, Step::Redo), "nothing from before the format is left to redo");
        assert_eq!(redone, now);
        // Typing after the format starts from what the page has: Undo brings back the row with the
        // alignment the format left it, and nothing older.
        edit(&mut history, &mut now, EditKind::Deleted, "A");
        assert!(history.step(&mut now, Step::Undo));
        assert_eq!(now.aligns, [None, Some(Right)]);
        assert!(!history.step(&mut now, Step::Undo), "no step reaches back before the format");
    }

    /// Undo back to a text the box showed brings its rows' alignments back, unless an alignment
    /// was picked since: then the older alignments are stale, and the edit is followed instead, so
    /// Undo never takes back the pick.
    #[test]
    fn typing_history_restores_alignments_unless_one_was_picked_since() {
        let mut now = typed("A\nB\nC", &[None, Some(Center), Some(Right)]);
        let mut history = TypingHistory::new(now.clone());
        edit(&mut history, &mut now, EditKind::Deleted, "A\nC");
        assert_eq!(now.aligns, [None, Some(Right)], "the deleted row's entry goes");
        assert!(history.step(&mut now, Step::Undo));
        assert_eq!(now.aligns, [None, Some(Center), Some(Right)], "Undo brings it back");
        edit(&mut history, &mut now, EditKind::Deleted, "A\nC");
        // An alignment picked for the first row, with no typing.
        now.aligns = vec![Some(Right), Some(Right)];
        history.pick_align();
        let picked = now.aligns.clone();
        assert!(history.step(&mut now, Step::Undo));
        assert_eq!(now.text, "A\nB\nC");
        assert_eq!(now.aligns, pdfcraft_engine::paragraph_align::remap("A\nC", "A\nB\nC", &picked), "the pick stays");
    }

    /// A run of typing is one step, a run of deleting another, and a paste (or a composition's
    /// commit) a step of its own: a wrong grouping undoes a word at a time, or a whole paragraph.
    #[test]
    fn the_typing_history_of_a_box_steps_a_run_as_one_gesture_and_a_paste_as_one_step() {
        let mut now = typed("", &[]);
        let mut history = TypingHistory::new(now.clone());
        for text in ["a", "ab", "abc"] {
            edit(&mut history, &mut now, EditKind::Typed, text);
        }
        edit(&mut history, &mut now, EditKind::Pasted, "abcdef");
        for text in ["abcde", "abcd"] {
            edit(&mut history, &mut now, EditKind::Deleted, text);
        }
        edit(&mut history, &mut now, EditKind::Composing, "abcdx");
        edit(&mut history, &mut now, EditKind::Committed, "abcdé");
        let mut undone = Vec::new();
        while history.step(&mut now, Step::Undo) {
            undone.push(now.text.clone());
        }
        assert_eq!(undone, ["abcd", "abcdef", "abc", ""], "the commit, the deleting, the paste, the typing");
        assert!(!history.can_undo() && history.can_redo());
        assert!(history.step(&mut now, Step::Redo));
        assert_eq!(now.text, "abc", "the typing comes back whole");
    }

    /// An app with one text document open, and an editor on its first paragraph.
    fn preview_app() -> (crate::PdfCraftApp, LineEditor) {
        let mut app = crate::PdfCraftApp::for_unit_tests();
        let bytes = app.session.create_from_text("Preview", "Preview text").unwrap();
        app.open_bytes(&egui::Context::default(), "preview.pdf", None, bytes.as_ref().clone()).unwrap();
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
            overflow: false,
            max_width: 400.0,
            size: block.size as f32,
            focus: true,
            look: look_of(&block),
            look0: look_of(&block),
            family_chosen: false,
            size_chosen: false,
            extras: Extras::default(),
            extras0: Extras::default(),
            history: TypingHistory::new(Typed::of(block.text.clone(), None, vec![None, None])),
        };
        (app, ed)
    }

    #[test]
    fn review_committing_the_shown_size_applies_it_to_a_mixed_selection() {
        let (_app, mut ed) = preview_app();
        ed.selection = Some(TextSelection::new(0, 2));
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
        let mut caret = Some(TextSelection::caret_at(1));
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
        app.views[0].session.line_editor = Some(ed);
        let inter = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf").to_vec();
        app.use_files(&egui::Context::default(), crate::files::FilePurpose::TextFont { page: 0 }, vec![("Inter-Regular.ttf".into(), inter)]);
        let doc = app.session.get(app.views[0].id).unwrap();
        let lines = doc.text_lines(0);
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), "Preview text");
        assert!(lines.iter().all(|l| l.base_font == "Inter-Regular"), "{:?}", lines.iter().map(|l| &l.base_font).collect::<Vec<_>>());
        assert!(app.views[0].session.line_editor.is_some(), "the paragraph stays open for editing");
    }

    #[test]
    fn save_applies_the_paragraph_open_in_the_editor() {
        // Save wrote the file without the paragraph still open in the editor (#3496).
        let (mut app, mut ed) = preview_app();
        ed.text = "Text preview".into();
        app.views[0].session.line_editor = Some(ed);

        // A new document asks where to save; the paragraph is applied before it asks.
        assert!(!app.save_active(&egui::Context::default(), crate::SaveTarget::InPlace));
        assert!(app.views[0].session.line_editor.is_none());
        let doc = app.session.get(app.views[0].id).unwrap();
        assert_eq!(doc.text_lines(0).iter().map(|l| l.text.as_str()).collect::<String>(), "Text preview");
    }

    #[test]
    fn a_refused_text_box_does_not_lose_the_open_paragraph() {
        let (mut app, mut ed) = preview_app();
        ed.text = "Text preview".into();
        app.views[0].session.line_editor = Some(ed);
        let draft = crate::content_ui::TextDraft::new(9, None, [20.0, 20.0, 120.0, 40.0], "Typed text".into(), crate::content_ui::default_style());
        app.views[0].session.content.draft = Some(draft.clone());

        assert!(!app.commit_typing_in(&egui::Context::default(), app.views[0].id));
        assert_eq!(app.views[0].session.content.draft, Some(draft), "the refused text box stays open");
        assert!(app.views[0].session.line_editor.is_none(), "the paragraph was applied");
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
        let mut app = crate::PdfCraftApp::for_unit_tests();
        let png = pdfcraft_engine::export::encode_png(200, 100, &[235u8; 200 * 100 * 4]).unwrap();
        let bytes = app.session.create_from_images(&[("scan.png".into(), png)]).unwrap();
        app.open_bytes(&egui::Context::default(), "scan.pdf", None, bytes.as_ref().clone()).unwrap();
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
            let job = editor_job(ui.ctx(), &ed, &BTreeMap::new(), &ed.text, 1.0, f32::INFINITY);
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
        app.views[0].session.line_editor = Some(ed.clone());

        assert!(!app.commit_typing_in(&egui::Context::default(), app.views[0].id));
        assert_eq!(app.views[0].session.line_editor, Some(ed), "the paragraph keeps its unsaved text");
    }

    #[test]
    fn text_background_is_cached_without_editing_and_released_on_cancel() {
        let (app, ed) = preview_app();
        let doc = app.session.get(app.views[0].id).unwrap();
        let source = doc.bytes.clone();
        let generation = doc.edit_generation();
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        let store = background::background_store(&ctx);
        let key = (generation, 0, TextTarget::Block(0));
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
        let revision = Revision::new(background.tag.unwrap());
        let TextureState::Ready(texture) = store.state(&ctx, &key, revision) else { panic!("the raster is in the store") };
        background.prepare(&ctx, doc, Some(&ed), None, 1.0);
        let TextureState::Ready(again) = store.state(&ctx, &key, revision) else { panic!("the raster stays in the store") };
        assert_eq!(again.id, texture.id, "a second prepare reuses the texture");
        assert_eq!(doc.bytes, source);
        assert_eq!(doc.edit_generation(), generation);
        assert!(!doc.dirty && doc.can_undo().is_none());
        background.prepare(&ctx, doc, None, None, 1.0);
        assert!(!background.ready(0, 0), "released when the paragraph closes");
        assert_eq!(store.state(&ctx, &key, revision), TextureState::Missing, "the store let go of the raster");
        ctx.end_pass().textures_delta.clear();
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
    fn superscript_and_subscript_conversion() {
        use pdfcraft_engine::text_script::Script;
        let (mut text, mut selection) = ("E = mc2".to_string(), Some(TextSelection::new(6, 7)));
        assert!(toggle_script(&mut text, &mut selection, Script::Super));
        assert_eq!((text.as_str(), selection), ("E = mc²", Some(TextSelection::new(6, 7))), "the selection follows the converted character");
        assert!(toggle_script(&mut text, &mut selection, Script::Super));
        assert_eq!((text.as_str(), selection), ("E = mc2", Some(TextSelection::new(6, 7))));
        // A backwards selection keeps its caret at the start.
        let (mut text, mut backwards) = ("a2b".to_string(), Some(TextSelection::new(2, 1)));
        assert!(toggle_script(&mut text, &mut backwards, Script::Super));
        assert_eq!(backwards, Some(TextSelection::new(2, 1)));

        // No selection: the whole text. H and O have no subscript form, and it still toggles back.
        let (mut text, mut none) = ("H2O".to_string(), None);
        assert!(toggle_script(&mut text, &mut none, Script::Sub));
        assert_eq!((text.as_str(), none), ("H₂O", None));
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
            source: None,
            multiline: false,
            size: 12.0,
            family_chosen: false,
            size_chosen: false,
            focus: true,
            look: pdfcraft_engine::AddedText::default(),
            look0: pdfcraft_engine::AddedText::default(),
            extras: Extras { line_spacing: 1.33, underline: true, ..Default::default() },
            extras0: Extras::default(),
            history: TypingHistory::new(Typed::of("Paragraph text".into(), None, Vec::new())),
        };
        let style = ed.style();
        assert_eq!(style.line_spacing, Some(1.33));
        assert_eq!(style.underline, Some(true));
    }
}
