//! The JavaScript editor: a code box that colours the script as it is typed, numbers its lines and
//! keeps the indentation. Colouring reads the text with a small tokenizer of our own
//! ([`pdfcraft_engine::js::highlight`]): nothing here runs a script or uses the engine's parser, so a
//! script from any document can be shown and edited safely.

mod edit;

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, CharIndex, LayoutJob, TextFormat};
use egui::{Color32, Stroke};
use pdfcraft_engine::js::{Token, TokenKind};

use crate::theme::{ThemeKind, Tokens};

/// Under the box's id, the indentation its Tab, Enter and `}` use: worked out from the script when
/// one of them is first needed, and again once a new script or a paste of indented code comes in.
const INDENT_KEY: &str = "script-indent";

/// The tallest the box can be dragged, in points.
const MAX_HEIGHT: f32 = 900.0;

/// Most tokens coloured; the rest of a longer script is plain. Code doesn't get there: a form script
/// at the 256 KiB limit is some 13,000 tokens, and minified some 30,000. A long table of values can
/// (`"abc",` at the limit is some 44,000). A script of `1,1,1,…` is 131,072, with as many plain
/// pieces between them, and each is a section the layout shapes on its own: its first layout took
/// 1.6 s, and every keystroke most of a second. This bounds that to about a quarter of a second.
const MAX_COLOURED_TOKENS: usize = 40_000;

/// Longest script the box shows or can make: the engine's own limit, past which it won't run anyway.
/// A longer one a document holds gets a note instead ([`too_long`]): a PDF's stream can decode to
/// tens of megabytes, and every frame would lay them all out. The box refuses an edit that would
/// pass it ([`Bounded`]).
const MAX_EDITED_BYTES: usize = pdfcraft_engine::js::MAX_SCRIPT_BYTES;

/// The text the box last left (by hash), and whether it has refused an edit ([`Bounded`]) since that
/// text changed.
#[derive(Clone, Default)]
struct Seen {
    text: u64,
    refused: bool,
}

/// The tokens of the text last coloured, so an unchanged script is not tokenized every frame.
#[derive(Clone, Default)]
struct Colours {
    hash: u64,
    tokens: Arc<Vec<Token>>,
}

/// The script as the text box edits it, which keeps it within [`MAX_EDITED_BYTES`]. Whatever the box
/// adds comes through here, however it came (typing, a paste, Enter, an input method, assistive
/// technology), and an addition that would pass the limit is refused whole: `refused` is set, and the
/// script stays as it was, with its undo history. What the editor adds itself (Enter, Tab) is held to
/// the same limit where it does that. So what the box leaves is never a script the engine won't run
/// for its length, and the note ([`too_long`]) is only ever for text the box did not type: a script a
/// document held, or a value typed for another kind of action before switching to a script.
///
/// It also keeps a `\r\n` whole where [`snap_off_crlf`] can't: a lone `\r` would be a line end of its
/// own. That moves the caret before an edit, but a paste can bring its own place, a click and typing
/// can share a frame, and a delete to the end of a line ends between the two.
struct Bounded<'a> {
    text: &'a mut String,
    refused: &'a mut bool,
    /// What the box last deleted as a selection, and where: it does that before it adds over one.
    selection: Option<(CharIndex, String)>,
}

impl egui::TextBuffer for Bounded<'_> {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        self.text
    }

    fn insert_text(&mut self, text: &str, char_index: CharIndex) -> usize {
        let selection = self.selection.take();
        if self.text.len().saturating_add(text.len()) <= MAX_EDITED_BYTES {
            // Over a selection the two can have just come together (`\r«x»\n`), and the text goes
            // where the selection was, between them, as two line ends they were.
            let replacing = selection.as_ref().is_some_and(|(at, deleted)| *at == char_index && !deleted.is_empty());
            if !replacing && edit::inside_crlf(self.text, char_index.0) {
                // Before the `\r`, and the caret, which egui moves on by what was added, after the text.
                return self.text.insert_text(text, CharIndex(char_index.0.saturating_sub(1))).saturating_sub(1);
            }
            return self.text.insert_text(text, char_index);
        }
        *self.refused = true;
        // The selection this was to replace goes back, and the caret after it.
        selection.filter(|(at, _)| *at == char_index).map_or(0, |(at, deleted)| self.text.insert_text(&deleted, at))
    }

    fn delete_char_range(&mut self, char_range: Range<CharIndex>) {
        let Range { start, end } = char_range;
        if start >= end {
            return;
        }
        let end = match edit::inside_crlf(self.text, end.0) {
            false => end,
            // All there is to delete to the end of this line is its line end: the lines join, as they do
            // at a `\n`.
            true if end.0.saturating_sub(1) == start.0 => CharIndex(end.0.saturating_add(1)),
            true => CharIndex(end.0.saturating_sub(1)),
        };
        self.text.delete_char_range(start..end);
    }

    fn delete_selected(&mut self, cursor_range: &CCursorRange) -> CCursor {
        let [min, max] = cursor_range.sorted_cursors();
        self.selection = Some((min.index, self.char_range(min.index..max.index).to_owned()));
        self.delete_selected_ccursor_range([min, max])
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Bounded<'static>>()
    }
}

/// A JavaScript box `rows` lines high and `width` wide (`f32::INFINITY` for all the room there
/// is), with `hint` shown while it is empty. `id_salt` names the text box as
/// [`egui::TextEdit::id_salt`] does. Returns the text box's response, or for a script too long to
/// show the response of the note in its place; `text` then changes only if the user clears it. The
/// box itself never takes `text` past the limit ([`Bounded`]).
pub(crate) fn script_editor(ui: &mut egui::Ui, text: &mut String, id_salt: &str, rows: usize, width: f32, hint: Option<&str>) -> egui::Response {
    // Named in the caller's `ui`, as `id_salt` alone would; what the box has to say goes under it,
    // also in a grid cell or a horizontal row.
    let id = ui.make_persistent_id(id_salt);
    ui.vertical(|ui| if text.len() > MAX_EDITED_BYTES { too_long(ui, text, width) } else { editor(ui, text, id, rows, width, hint) }).inner
}

/// In place of the box, for a script past [`MAX_EDITED_BYTES`], which the box itself never makes.
/// Clearing is the way to write another in its place: there is no box to delete it in.
fn too_long(ui: &mut egui::Ui, text: &mut String, width: f32) -> egui::Response {
    if width.is_finite() {
        ui.set_max_width(width);
    }
    let note = crate::i18n::fmt(
        tl!("This script is {size}, too long to edit or run here (the limit is {limit}). It is left as it is."),
        &[("size", &crate::panels::human_size(text.len())), ("limit", &crate::panels::human_size(MAX_EDITED_BYTES))],
    );
    let error = Palette::for_kind(Tokens::get(ui.ctx()).kind).error;
    let response = ui.label(egui::RichText::new(note).color(error));
    if ui.button(tl!("Clear script")).clicked() {
        *text = String::new();
    }
    response
}

fn editor(ui: &mut egui::Ui, text: &mut String, id: egui::Id, rows: usize, width: f32, hint: Option<&str>) -> egui::Response {
    let (seen_id, colours_id, height_id) = (id.with("script-seen"), id.with("script-colours"), id.with("script-height"));
    let now = ui.input(|i| i.time);
    let stored: Option<Seen> = ui.data(|d| d.get_temp(seen_id));
    let arrived = hash(text);
    // Text the box did not type itself (another field's script loaded into the same box) starts its
    // own undo history: Cmd+Z must not bring back the script shown before it. It has its own
    // indentation too. Told apart by content: one identical to what the box left keeps the history.
    let outside = stored.as_ref().is_none_or(|s| s.text != arrived);
    if outside {
        ui.data_mut(|d| d.remove_temp::<String>(id.with(INDENT_KEY)));
        if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), id) {
            let mut undoer = egui::util::undoer::Undoer::<(CCursorRange, String)>::default();
            undoer.feed_state(now, &(state.cursor.char_range().unwrap_or_default(), text.clone()));
            state.set_undoer(undoer);
            state.store(ui.ctx(), id);
        }
    }
    let mut seen = stored.filter(|_| !outside).unwrap_or(Seen { text: arrived, refused: false });
    let mut colours: Colours = ui.data(|d| d.get_temp(colours_id)).unwrap_or_else(|| Colours { hash: hash(""), tokens: Arc::default() });
    let palette = Palette::for_kind(Tokens::get(ui.ctx()).kind);
    let font = egui::TextStyle::Monospace.resolve(ui.style());

    let Keys { edited, mut refused } = editor_keys(ui, text, id);

    // The box: coloured, in a frame that can be dragged taller.
    let digits = edit::line_count(text).to_string().len().max(2);
    let digit_width = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let gutter = digit_width * digits as f32 + 12.0;
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
        let source = buffer.as_str();
        let h = hash(source);
        if colours.hash != h {
            colours = Colours { hash: h, tokens: Arc::new(pdfcraft_engine::js::highlight(source)) };
        }
        let mut job = layout_job(source, &colours.tokens, &palette, &egui::TextStyle::Monospace.resolve(ui.style()));
        job.wrap.max_width = wrap_width;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    let box_width = if width.is_finite() { width } else { ui.available_width() };
    let min_height = row_height * 2.0 + 14.0;
    // In a dialog, no taller than the dialog can grow and still fit in the window: a box dragged
    // past that would push the dialog's own buttons off the screen. The dialog's size is last
    // frame's, so it is used only if the box was in it then (a box just opened could be measured
    // against another dialog, and the box keeps a height it is clamped to).
    let frame = ui.ctx().cumulative_frame_nr();
    let last = ui.data(|d| d.get_temp::<(u64, f32)>(height_id)).filter(|(at, _)| at.saturating_add(1) >= frame).map(|(_, h)| h);
    // Not there last frame, it has just opened, and a refusal from the last time isn't news.
    if last.is_none() {
        seen.refused = false;
    }
    let max_height = match (crate::widgets::dialog_room(ui), last) {
        (Some(room), Some(last)) if (last + room).is_finite() => (last + room).clamp(min_height, MAX_HEIGHT.max(min_height)),
        _ => MAX_HEIGHT,
    };
    let mut output: Option<egui::text_edit::TextEditOutput> = None;
    // The part of the box that shows: the gutter is clipped to it.
    let mut viewport = egui::Rect::NOTHING;
    // How tall the box is, whether or not it all shows: partly off the screen, what shows is less.
    let mut height = f32::NAN;
    // The whole box. The frame goes on this rather than the text box, which is as long as its text
    // and would scroll its edges out of sight.
    let mut outer = egui::Rect::NOTHING;
    let corner = ui.visuals().widgets.inactive.corner_radius;
    let fill = ui.visuals().text_edit_bg_color();
    egui::Resize::default()
        .id_salt(id.with("size"))
        .resizable([false, true])
        .default_width(box_width)
        .min_width(box_width)
        // The grip applies the pointer to both directions whatever `resizable` says.
        .max_width(box_width)
        .default_height(row_height * rows.max(1) as f32 + 14.0)
        .min_height(min_height)
        .max_height(max_height)
        .with_stroke(false)
        .show(ui, |ui| {
            outer = ui.max_rect();
            ui.painter().rect_filled(outer, corner, fill);
            egui::ScrollArea::vertical().id_salt(id.with("scroll")).auto_shrink([false, false]).show(ui, |ui| {
                viewport = ui.clip_rect();
                height = ui.available_height();
                let margin = egui::Margin { left: (10.0 + gutter).round().clamp(0.0, 120.0) as i8, right: 10, top: 6, bottom: 6 };
                let mut bounded = Bounded { text: &mut *text, refused: &mut refused, selection: None };
                let mut edit = crate::widgets::block(&mut bounded)
                    .code_editor()
                    .frame(egui::Frame::NONE.inner_margin(margin))
                    // As tall as the box, set below: its own four rows would scroll a short box for nothing.
                    .desired_rows(1)
                    .desired_width(f32::INFINITY)
                    .min_size(egui::vec2(0.0, ui.available_height()))
                    .id(id)
                    .layouter(&mut layouter);
                if let Some(hint) = hint {
                    edit = edit.hint_text(hint);
                }
                let shown = edit.show(ui);
                // The editor moved the caret itself above: the text box does not see it move, so
                // the box is scrolled to it here.
                if edited && let Some(range) = shown.cursor_range {
                    let caret = shown.galley.pos_from_cursor(range.primary).translate(shown.galley_pos.to_vec2());
                    ui.scroll_to_rect(caret.expand(4.0), None);
                }
                output = Some(shown);
            });
        });
    let Some(output) = output else { return ui.label("") };
    if height.is_finite() {
        ui.data_mut(|d| d.insert_temp(height_id, (frame, height)));
    }
    let mut response = output.response.response.clone().accessible_name(tl!("JavaScript"));
    if edited {
        response.mark_changed();
    }
    paint_gutter(ui, &output, text, gutter, &font, viewport);
    // As a text box's: the selection's colour while focused. Over the text, so nothing scrolls across it.
    let stroke = if response.has_focus() { ui.visuals().selection.stroke } else { ui.style().interact(&response).bg_stroke };
    ui.painter().rect_stroke(outer, corner, stroke, egui::StrokeKind::Inside);

    // Said until the script changes: an edit the box refused, which would otherwise look like nothing.
    let h = hash(text);
    if h != seen.text {
        seen = Seen { text: h, refused: false };
    }
    seen.refused |= refused;
    if seen.refused {
        let said = crate::i18n::fmt(
            tl!("Not added: the script would be longer than the {limit} limit."),
            &[("limit", &crate::panels::human_size(MAX_EDITED_BYTES))],
        );
        ui.label(egui::RichText::new(said).small().color(palette.error));
    }
    ui.data_mut(|d| {
        d.insert_temp(seen_id, seen);
        d.insert_temp(colours_id, colours);
    });
    response
}

/// What [`editor_keys`] did in a frame.
#[derive(Default)]
struct Keys {
    /// The text was edited.
    edited: bool,
    /// What the editor would have added was refused, as past [`MAX_EDITED_BYTES`]. It doesn't go through
    /// the text box, whose own limit ([`Bounded`]) is for what the box adds.
    refused: bool,
}

/// The keys the editor handles itself, before the text box sees them: Tab and Shift+Tab, Enter and
/// a typed `}`, one to a frame ([`hold_until_their_turn`]).
fn editor_keys(ui: &mut egui::Ui, text: &mut String, id: egui::Id) -> Keys {
    let mut keys = Keys::default();
    let held_id = id.with("script-held-keys");
    let held: Vec<egui::Event> = ui.data_mut(|d| d.remove_temp(held_id)).unwrap_or_default();
    if !ui.memory(|m| m.has_focus(id)) {
        // What was held was typed for the box: it does not go anywhere else, or wait for the box's return.
        return keys;
    }
    let later = ui.input_mut(|i| hold_until_their_turn(&mut i.events, held));
    if !later.is_empty() {
        ui.data_mut(|d| d.insert_temp(held_id, later));
        ui.ctx().request_repaint();
    }
    let indent_id = id.with(INDENT_KEY);
    // Code pasted in with indentation of its own sets it. What the editor adds itself must not count,
    // or Tab would grow with each press and Shift+Tab take off more than Tab added.
    if ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Paste(pasted) if edit::has_indented_line(pasted)))) {
        ui.data_mut(|d| d.remove_temp::<String>(indent_id));
    }
    // On what edits there (typing, Backspace or Delete would split the line end), and on nothing
    // else. Not on the keys that only move: moved back every frame, Right could never cross a line
    // end. Not on Undo and Redo, which go by whether the caret is still where the undo point has it:
    // moved, Undo would go back to that same point every time. And not on Copy, a Cut with nothing
    // selected, or a shortcut that isn't for editing: moving the caret for those would lose the Redo
    // that Undo left.
    copy_off_crlf(ui, id, text);
    let selected = selection(ui.ctx(), id).is_some_and(|s| !s.is_empty());
    let editing = ui.input(|i| {
        i.events.iter().any(|e| match e {
            egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Ime(_) => true,
            egui::Event::Cut => selected,
            egui::Event::Key { key, pressed: true, modifiers, .. } => {
                deletes(*key, *modifiers) || (matches!(key, egui::Key::Enter | egui::Key::Tab) && !modifiers.ctrl && !modifiers.command)
            }
            _ => false,
        })
    });
    if editing {
        snap_off_crlf(ui.ctx(), id, text);
    }
    let Some(selection) = selection(ui.ctx(), id) else { return keys };
    let backward = selected_backward(ui.ctx(), id);
    let (tab, shift_tab, enter) = ui.input_mut(|i| {
        let none = egui::Modifiers::NONE;
        // The box would type a tab over the selection for Ctrl+Tab: a key that is not typing.
        i.events
            .retain(|e| !matches!(e, egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } if modifiers.ctrl || modifiers.command));
        // Shift+Tab first: a plain key also matches when Shift is held.
        let shift_tab = i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab);
        (i.consume_key(none, egui::Key::Tab), shift_tab, i.consume_key(none, egui::Key::Enter))
    });
    // Only a `}` typed on its own: one among other edits this frame goes in with them, in order. By
    // AltGr, as it is on many layouts, it can follow a key reported as Ctrl and Alt. From an input
    // method it goes in as it comes.
    let is_brace = |e: &egui::Event| matches!(e, egui::Event::Text(t) if t == "}");
    let brace = ui.input(|i| {
        let mut acting = i.events.iter().filter(|e| acts_on_text(e) && !is_alt_gr(e));
        acting.next().is_some_and(is_brace) && acting.next().is_none()
    });
    if !(tab || shift_tab || enter || brace) {
        return keys;
    }
    let unit = ui.data(|d| d.get_temp::<String>(indent_id)).unwrap_or_else(|| edit::indent_unit(text));
    let done = if shift_tab {
        Some(edit::shift_tab(text, selection.clone(), &unit))
    } else if tab {
        Some(edit::tab(text, selection.clone(), &unit))
    } else if enter {
        // White space in a string or template is what it holds, so nothing is added.
        Some(if edit::in_literal(text, selection.start) {
            edit::line_break(text, selection.clone(), "")
        } else {
            edit::newline(text, selection.clone(), &unit)
        })
    } else if edit::in_literal(text, selection.start) {
        None
    } else {
        edit::close_brace(text, selection.clone(), &unit)
    };
    let Some(done) = done else { return keys };
    // A Shift+Tab with nothing to take off: touching the selection would lose the Redo that Undo left.
    // Enter over a selected line end leaves the same text too, but it moves on past it.
    if done.text == *text && done.selection == selection {
        return keys;
    }
    ui.data_mut(|d| d.insert_temp(indent_id, unit));
    if brace {
        ui.input_mut(|i| i.events.retain(|e| !is_brace(e)));
    }
    if done.text.len() > MAX_EDITED_BYTES {
        keys.refused = true;
    } else {
        *text = done.text;
        set_selection_facing(ui.ctx(), id, done.selection, backward);
        keys.edited = true;
    }
    keys
}

/// Whether this is Undo or Redo, as the text box takes them.
fn undo_or_redo(key: egui::Key, modifiers: egui::Modifiers) -> bool {
    matches!(key, egui::Key::Z | egui::Key::Y) && modifiers.matches_logically(egui::Modifiers::COMMAND)
}

/// Whether the box or the editor does something to the text or the caret for `event`.
fn acts_on_text(event: &egui::Event) -> bool {
    match event {
        egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Cut | egui::Event::Copy | egui::Event::Ime(_) => true,
        egui::Event::Key { key, pressed: true, modifiers, .. } => {
            modifiers.command
                || modifiers.ctrl
                || matches!(
                    key,
                    egui::Key::Backspace
                        | egui::Key::Delete
                        | egui::Key::Enter
                        | egui::Key::Tab
                        | egui::Key::ArrowLeft
                        | egui::Key::ArrowRight
                        | egui::Key::ArrowUp
                        | egui::Key::ArrowDown
                        | egui::Key::Home
                        | egui::Key::End
                )
        }
        _ => false,
    }
}

/// Whether the editor takes `event` out before the box sees the frame's events, or it needs to be
/// alone in its frame: Undo and Redo, which [`snap_off_crlf`] must not come near.
fn taken_out_of_turn(event: &egui::Event) -> bool {
    match event {
        egui::Event::Key { key, pressed: true, modifiers, .. } => matches!(key, egui::Key::Enter | egui::Key::Tab) || undo_or_redo(*key, *modifiers),
        _ => false,
    }
}

/// Whether `event` moves the caret and does nothing else, which can leave it inside a line end: the
/// keys for it, and on a Mac, where Ctrl isn't Command, the Emacs-like ones.
fn moves_caret(event: &egui::Event) -> bool {
    let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { return false };
    match key {
        egui::Key::ArrowLeft | egui::Key::ArrowRight | egui::Key::ArrowUp | egui::Key::ArrowDown | egui::Key::Home | egui::Key::End => true,
        egui::Key::A | egui::Key::B | egui::Key::E | egui::Key::F | egui::Key::N | egui::Key::P => {
            modifiers.ctrl && !modifiers.command && !modifiers.shift
        }
        _ => false,
    }
}

/// Whether the box deletes for this key: Backspace, Delete and egui's Emacs-like deletes.
fn deletes(key: egui::Key, modifiers: egui::Modifiers) -> bool {
    match key {
        egui::Key::Backspace | egui::Key::Delete => true,
        egui::Key::H | egui::Key::K | egui::Key::U | egui::Key::W => modifiers.ctrl,
        _ => false,
    }
}

/// Whether `event` is a key reported with both Ctrl and Alt, as AltGr can be, and which doesn't delete:
/// it types, or does nothing.
fn is_alt_gr(event: &egui::Event) -> bool {
    matches!(event, egui::Event::Key { key, modifiers, .. } if modifiers.ctrl && modifiers.alt && !deletes(*key, *modifiers))
}

/// Keeps the order of a frame's keys when frames are slow and several come at once. The editor takes
/// its keys before the box sees the frame's events, so one that comes after an edit would act first,
/// and two of them would count as one. This leaves in `events` what can go in this frame, after
/// `held` from the last one, and returns the rest for the next: a key the editor takes goes alone,
/// with only what doesn't touch the text, and an edit waits behind keys that move the caret, so that
/// [`snap_off_crlf`] sees where they left it. Everything else, a typed word above all, stays together
/// in its order.
///
/// A key held down that repeats is dropped rather than held: frames too slow for it would have it
/// go on long after the key is up.
fn hold_until_their_turn(events: &mut Vec<egui::Event>, mut held: Vec<egui::Event>) -> Vec<egui::Event> {
    held.append(events);
    let wait_from = {
        let (mut first, mut last_took, mut moved) = (true, false, false);
        held.iter().enumerate().filter(|(_, e)| acts_on_text(e)).find_map(|(at, e)| {
            let waits = last_took || (!first && taken_out_of_turn(e)) || (moved && !moves_caret(e));
            last_took = first && taken_out_of_turn(e);
            moved |= moves_caret(e);
            first = false;
            waits.then_some(at)
        })
    };
    let mut later = Vec::new();
    for (at, event) in held.into_iter().enumerate() {
        if !(wait_from.is_some_and(|from| at >= from) && acts_on_text(&event)) {
            events.push(event);
        } else if !matches!(event, egui::Event::Key { repeat: true, .. }) {
            later.push(event);
        }
    }
    later
}

/// The selection of text box `id`, in characters, from its stored state.
fn selection(ctx: &egui::Context, id: egui::Id) -> Option<Range<usize>> {
    let state = egui::text_edit::TextEditState::load(ctx, id)?;
    let range = state.cursor.char_range()?;
    let [a, b] = range.sorted_cursors();
    Some(a.index.0..b.index.0)
}

/// A caret (or selection end) egui put between the `\r` and the `\n` of a line end, as End and a
/// click past a line's end do, moved before the `\r`: Enter, Tab and typing there would split it.
fn snap_off_crlf(ctx: &egui::Context, id: egui::Id, text: &str) {
    let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) else { return };
    let Some(range) = state.cursor.char_range() else { return };
    let (primary, secondary) = off_crlf(text, range);
    if (primary, secondary) != (range.primary, range.secondary) {
        state.cursor.set_char_range(Some(CCursorRange::two(secondary, primary)));
        state.store(ctx, id);
    }
}

/// The caret and the other end of `range`, neither inside a line end. A selection of only the `\r`
/// would become a caret, and an edit act on what is before it: the line end is what was selected.
fn off_crlf(text: &str, range: CCursorRange) -> (CCursor, CCursor) {
    let snap = |c: CCursor| if edit::inside_crlf(text, c.index.0) { CCursor::new(c.index.0.saturating_sub(1)) } else { c };
    let (primary, secondary) = (snap(range.primary), snap(range.secondary));
    if primary != secondary || range.primary == range.secondary {
        return (primary, secondary);
    }
    let end = CCursor::new(primary.index.0.saturating_add(2));
    if range.primary.index > range.secondary.index { (end, secondary) } else { (primary, end) }
}

/// For a Copy next among this frame's events, with a selection that ends inside a line end: copies it
/// without half of that line end, as a lone `\r` pasted elsewhere is a line break. It takes the Copy
/// from the box, and leaves the selection where it is, as moving it would lose the Redo that Undo left.
/// Next, whatever shortcuts come first that leave the selection alone: a browser sends Cmd+C itself
/// ahead of its Copy. Select All and keys that delete don't, and what moves the caret waits its turn
/// ([`hold_until_their_turn`]).
fn copy_off_crlf(ui: &egui::Ui, id: egui::Id, text: &str) {
    let leaves_selection =
        |e: &egui::Event| matches!(e, egui::Event::Key { key, modifiers, .. } if *key != egui::Key::A && !deletes(*key, *modifiers));
    if !ui.input(|i| matches!(i.events.iter().filter(|e| acts_on_text(e)).find(|e| !leaves_selection(e)), Some(egui::Event::Copy))) {
        return;
    }
    let Some(range) = egui::text_edit::TextEditState::load(ui.ctx(), id).and_then(|s| s.cursor.char_range()) else { return };
    let (primary, secondary) = off_crlf(text, range);
    if range.primary == range.secondary || (primary, secondary) == (range.primary, range.secondary) {
        return;
    }
    let (from, to) = (primary.index.0.min(secondary.index.0), primary.index.0.max(secondary.index.0));
    ui.ctx().copy_text(text.get(edit::byte_of(text, from)..edit::byte_of(text, to)).unwrap_or_default().to_owned());
    ui.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Copy)));
}

/// Whether the selection of text box `id` was made backward (its caret at the start).
fn selected_backward(ctx: &egui::Context, id: egui::Id) -> bool {
    egui::text_edit::TextEditState::load(ctx, id).and_then(|s| s.cursor.char_range()).is_some_and(|r| r.primary.index.0 < r.secondary.index.0)
}

/// Put the selection of text box `id` on `selection`, its caret at the start when `backward`.
fn set_selection_facing(ctx: &egui::Context, id: egui::Id, selection: Range<usize>, backward: bool) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let (start, end) = (CCursor::new(selection.start), CCursor::new(selection.end));
    let range = if backward { CCursorRange::two(end, start) } else { CCursorRange::two(start, end) };
    state.cursor.set_char_range(Some(range));
    state.store(ctx, id);
}

/// Line numbers in the box's left margin, by the first row of each line (a wrapped line has one).
/// Only the rows in `viewport`, the part of the scrolled box that shows, are numbered. A row
/// starts a line when the one before ends with a line end of `text`: a `\n`, a lone `\r` (laid out
/// as one, [`layout_job`]), or U+2028 or U+2029 if wrapping happens to stop after it. egui breaks
/// rows at neither of those last two by itself, so the line after one in mid-row has no number.
fn paint_gutter(ui: &egui::Ui, output: &egui::text_edit::TextEditOutput, text: &str, gutter: f32, font: &egui::FontId, viewport: egui::Rect) {
    let t = Tokens::get(ui.ctx());
    let rect = output.response.response.rect.intersect(viewport);
    let painter = ui.painter_at(rect);
    let right = rect.left() + 4.0 + gutter;
    let mut line = 1usize;
    let mut starts_line = true;
    let mut chars = text.chars();
    for row in &output.galley.rows {
        let y = output.galley_pos.y + row.rect().min.y;
        if starts_line && y + row.rect().height() > rect.top() && y < rect.bottom() {
            painter.text(egui::pos2(right - 8.0, y), egui::Align2::RIGHT_TOP, line.to_string(), font.clone(), t.text_muted);
        }
        if starts_line {
            line = line.saturating_add(1);
        }
        let last = chars.by_ref().take(row.char_count_including_newline().0).last();
        starts_line = last.is_some_and(edit::is_line_end);
    }
    painter.vline(right, rect.y_range(), Stroke::new(1.0, t.divider));
}

fn hash(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// The colours of the script's tokens, and of what the box says when something is wrong.
#[derive(Clone, Copy)]
struct Palette {
    text: Color32,
    keyword: Color32,
    literal: Color32,
    string: Color32,
    regex: Color32,
    comment: Color32,
    api: Color32,
    error: Color32,
}

impl Palette {
    /// Each token's colour keeps at least 4.5:1 contrast on the theme's field background, and the
    /// error's on the dialog card it is written on (egui's own error red does not).
    fn for_kind(kind: ThemeKind) -> Self {
        let t = Tokens::for_kind(kind);
        match kind {
            ThemeKind::Light => Self {
                text: t.text,
                keyword: Color32::from_rgb(0x7A, 0x3E, 0x9D),
                literal: Color32::from_rgb(0x0B, 0x6E, 0x99),
                string: Color32::from_rgb(0x2E, 0x7D, 0x32),
                regex: Color32::from_rgb(0xA8, 0x51, 0x00),
                comment: Color32::from_rgb(0x6B, 0x70, 0x7A),
                api: Color32::from_rgb(0x1F, 0x5F, 0xBF),
                error: Color32::from_rgb(0xC6, 0x28, 0x28),
            },
            ThemeKind::Dark => Self {
                text: t.text,
                keyword: Color32::from_rgb(0xC7, 0x92, 0xEA),
                literal: Color32::from_rgb(0x7F, 0xD1, 0xF0),
                string: Color32::from_rgb(0x9C, 0xCC, 0x65),
                regex: Color32::from_rgb(0xF0, 0xA3, 0x5C),
                comment: Color32::from_rgb(0x9A, 0xA1, 0xAD),
                api: Color32::from_rgb(0x82, 0xAA, 0xFF),
                error: Color32::from_rgb(0xFF, 0x7B, 0x72),
            },
        }
    }

    fn of(&self, kind: TokenKind) -> Color32 {
        match kind {
            TokenKind::Keyword => self.keyword,
            TokenKind::Literal | TokenKind::Number => self.literal,
            TokenKind::String | TokenKind::Template => self.string,
            TokenKind::Regex => self.regex,
            TokenKind::Comment => self.comment,
            TokenKind::Api => self.api,
        }
    }
}

/// `source` laid out in `font`, its first [`MAX_COLOURED_TOKENS`] `tokens` coloured. A lone
/// `\r` (Acrobat's line end) is laid out as a `\n`: egui starts a row only at `\n`. Both are one
/// byte and one character, so every place in the layout is still that place in the script, as
/// egui's own password boxes lay out other characters than they hold.
fn layout_job(source: &str, tokens: &[Token], palette: &Palette, font: &egui::FontId) -> LayoutJob {
    let shown: std::borrow::Cow<'_, str> = if source.contains('\r') {
        let mut chars = source.chars().peekable();
        let mut out = String::with_capacity(source.len());
        while let Some(c) = chars.next() {
            out.push(if c == '\r' && chars.peek() != Some(&'\n') { '\n' } else { c });
        }
        out.into()
    } else {
        source.into()
    };
    let source = shown.as_ref();
    let tokens = tokens.get(..MAX_COLOURED_TOKENS).unwrap_or(tokens);
    let mut job = LayoutJob::default();
    // Cut points: where a token starts or ends. Every token sits on character boundaries, so
    // every cut does.
    let mut cuts: Vec<usize> = Vec::with_capacity(tokens.len().saturating_mul(2).saturating_add(4));
    cuts.extend([0, source.len()]);
    for t in tokens {
        cuts.extend([t.bytes.start, t.bytes.end]);
    }
    cuts.retain(|c| *c <= source.len() && source.is_char_boundary(*c));
    cuts.sort_unstable();
    cuts.dedup();
    let mut token = tokens.iter().peekable();
    for pair in cuts.windows(2) {
        let [from, to] = pair else { continue };
        let Some(piece) = source.get(*from..*to) else { continue };
        while token.next_if(|t| t.bytes.end <= *from).is_some() {}
        let kind = token.peek().filter(|t| t.bytes.start <= *from).map(|t| t.kind);
        let mut format = TextFormat { font_id: font.clone(), color: kind.map_or(palette.text, |k| palette.of(k)), ..Default::default() };
        format.italics = kind == Some(TokenKind::Comment);
        job.append(piece, 0.0, format);
    }
    job
}

#[cfg(test)]
mod tests;
