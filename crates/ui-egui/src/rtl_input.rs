//! The find bar and the Search panel type Arabic and Hebrew in reading order.
//!
//! egui shapes an Arabic word from the right, but it also emits a second, zero-width glyph for
//! the same letter and walks the caret through those duplicates. Measured on
//! `مؤشرات الأداء`: caret positions 9 through 13 all sat on the space, so the second word
//! never moved the caret, and the two words were drawn in logical order. The stored query stays
//! in typing order, because that is what search matches. The field shows the Unicode bidi
//! visual order, with one glyph per character sorted left to right, and the caret sits on the
//! leading edge of that line.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use egui::epaint::text::{CharIndex, LayoutJob};
use egui::text::CCursor;
use egui::{Galley, TextBuffer};
use unicode_bidi::{BidiInfo, Level};

/// Above this, the field keeps the characters but does not reorder them. A search phrase is
/// short; the cap stops a pasted document from building a giant bidi map.
const MAX_CHARS: usize = 100_000;

/// Screen strings kept so undo can turn egui's on-screen snapshot back into the typed query.
///
/// egui groups keystrokes into one undo point until the text sits still for a second, or for
/// 30 seconds if it never does. A held key repeats much faster than a person types, so one undo
/// point can cover far more than egui's 100 stored points. 4096 is that whole burst, twice over.
const SNAPSHOT_CAP: usize = 4096;

/// Screen text egui has already shown, paired with the typed query at that moment.
///
/// egui clones temp data on every read, so the history is an `Arc`. An unchanged frame
/// copies the pointer. A new edit copies the list once.
type UndoSnapshots = std::sync::Arc<std::collections::VecDeque<(String, String)>>;

/// Which single-line search field is on screen. The two share one buffer so they cannot drift.
#[derive(Clone, Copy)]
pub(crate) enum QueryField {
    /// The floating bar at the top of the page.
    FindBar,
    /// Edit ▸ Advanced Search.
    SearchPanel,
}

/// Draw `text` for a search field. Right-to-left text comes back with one glyph per character
/// in display order, so a caret index is a position on the line.
pub(crate) fn layout_galley(ui: &egui::Ui, text: &str, _wrap: f32) -> std::sync::Arc<Galley> {
    let font_id = egui::FontSelection::default().resolve(ui.style());
    let row_height = ui.fonts_mut(|f| f.row_height(&font_id));
    let line_height = row_height + ui.spacing().extra_text_line_spacing;
    let color = ui.visuals().override_text_color.unwrap_or_else(|| ui.visuals().widgets.inactive.text_color());
    let count = text.chars().count();
    // Past the cap the buffer stays in stored order, so the galley must too. Otherwise the caret
    // index and the glyph index name different characters.
    let screen = if count == 0 || count > MAX_CHARS || !crate::bidi::has_rtl(text) { None } else { Some(analyze(text)) };
    // The shaper string is the same characters as the field shows, including spaces and marks.
    // `visual_auto` drops a trailing space, and the caret then falls back to the frozen galley.
    let shaped = match &screen {
        Some(screen) => crate::bidi::for_shaper(&screen.text),
        None => text.to_owned(),
    };
    // Single-line, like egui's own field: do not wrap. The widget clips what does not fit.
    let mut job = LayoutJob::simple_singleline(shaped, font_id, color);
    job.keep_trailing_whitespace = true;
    for section in &mut job.sections {
        section.format.line_height = Some(line_height);
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let Some(screen) = screen else { return galley };
    let mut galley = std::sync::Arc::unwrap_or_clone(galley);
    place_caret_glyphs(&mut galley, screen.text.chars().count());
    std::sync::Arc::new(galley)
}

/// Replace each row's glyph list with one glyph per character, left to right.
///
/// egui's caret uses glyph index as character index. Arabic currently produces about two glyphs
/// per letter, and the extras have no width and share one x, so the caret freezes. A combining
/// mark or a direction mark is only a zero-width glyph, so the row is padded or trimmed until
/// the glyph count is the character count. Leaving the raw galley in place is what froze the caret.
fn place_caret_glyphs(galley: &mut Galley, chars: usize) {
    if chars == 0 {
        return;
    }
    let mut raw = Vec::new();
    for row in &galley.rows {
        raw.extend(row.glyphs.iter().copied());
    }
    // A letter is often emitted twice: a zero-width copy and the real glyph. A combining mark
    // is only the zero-width glyph, so it has to stay or the caret count no longer matches.
    let mut kept = Vec::new();
    for (i, glyph) in raw.iter().enumerate() {
        if glyph.advance_width <= 0.0 {
            let same = |other: Option<&egui::epaint::text::Glyph>| other.is_some_and(|g| g.chr == glyph.chr && g.advance_width > 0.0);
            if same(raw.get(i.saturating_add(1))) || (i > 0 && same(raw.get(i - 1))) {
                continue;
            }
        }
        kept.push(*glyph);
    }
    let drawn: std::collections::HashSet<char> = kept.iter().filter(|g| g.advance_width > 0.0).map(|g| g.chr).collect();
    kept.retain(|glyph| glyph.advance_width > 0.0 || !drawn.contains(&glyph.chr));
    // Still more slots than characters: drop leftover zero-width copies, then the extras.
    if kept.len() > chars {
        let mut i = kept.len();
        while kept.len() > chars && i > 0 {
            i -= 1;
            if kept.get(i).is_some_and(|glyph| glyph.advance_width <= 0.0) {
                kept.remove(i);
            }
        }
    }
    kept.sort_by(|a, b| a.pos.x.total_cmp(&b.pos.x).then(a.chr.cmp(&b.chr)));
    if kept.len() > chars {
        kept.truncate(chars);
    }
    while kept.len() < chars {
        let Some(template) = kept.last().copied() else { return };
        let mut pad = template;
        pad.advance_width = 0.0;
        pad.chr = '\u{200B}';
        kept.push(pad);
    }
    if kept.is_empty() {
        return;
    }
    let Some(row) = galley.rows.first_mut() else { return };
    let glyphs = std::sync::Arc::make_mut(&mut row.row);
    glyphs.glyphs = kept;
    // One visual line. Extra rows would make the caret count characters twice.
    galley.rows.truncate(1);
}

/// The search field. `text` stays in typing order.
pub(crate) fn show_query(ui: &mut egui::Ui, text: &mut String, kind: QueryField) -> egui::Response {
    let id = match kind {
        QueryField::FindBar => egui::Id::new("find-input"),
        QueryField::SearchPanel => egui::Id::new("search-panel-input"),
    };
    // egui's undo stores the on-screen string from an earlier frame. The map from that string
    // back to the typed query has to outlive this widget, which is built again every frame.
    let snapshots =
        ui.data(|d| d.get_temp::<UndoSnapshots>(id.with("rtl-undo"))).unwrap_or_else(|| std::sync::Arc::new(std::collections::VecDeque::new()));
    let logical = Rc::new(RefCell::new(text.clone()));
    let mut edit = RtlEdit::new(Rc::clone(&logical), snapshots);
    let shared = Rc::clone(&logical);
    let mut layouter = move |ui: &egui::Ui, _: &dyn TextBuffer, wrap: f32| layout_galley(ui, &shared.borrow(), wrap);
    // A new field puts the caret at the end of the buffer. For right-to-left text that end is
    // the start of the stored phrase, so the caret starts on the leading edge instead.
    let start_at_end = !edit.screen.base_rtl;
    let mut field = match kind {
        QueryField::FindBar => egui::TextEdit::singleline(&mut edit).id(id).hint_text(tl!("Find text")).desired_width(220.0).frame(egui::Frame::NONE),
        QueryField::SearchPanel => crate::widgets::line(&mut edit).id(id).hint_text(tl!("Search")).desired_width(f32::INFINITY),
    };
    field = field.layouter(&mut layouter).cursor_at_end(start_at_end);
    let output = field.show(ui);
    let focused = output.response.has_focus();
    if focused {
        let (copy, cut) =
            ui.input(|i| (i.events.iter().any(|e| matches!(e, egui::Event::Copy)), i.events.iter().any(|e| matches!(e, egui::Event::Cut))));
        if cut {
            if let Some(removed) = edit.deleted.clone() {
                ui.ctx().copy_text(removed);
            }
        } else if copy && let Some(range) = output.cursor_range {
            let chars = range.as_sorted_char_range();
            if chars.start.0 < chars.end.0 {
                ui.ctx().copy_text(edit.logical_screen_slice(chars.start.0..chars.end.0));
            }
        }
    }
    *text = logical.borrow().clone();
    ui.data_mut(|d| d.insert_temp(id.with("rtl-undo"), std::sync::Arc::clone(&edit.snapshots)));
    output.response.response
}

/// What the field shows, and how each shown character maps back to the stored text.
struct Screen {
    text: String,
    /// `order[shown character] = stored character index`.
    order: Vec<usize>,
    /// Whether that shown character is in a right-to-left run.
    rtl: Vec<bool>,
    /// The paragraph's base direction.
    base_rtl: bool,
}

#[cfg(test)]
fn screen_of(text: &str) -> String {
    analyze(text).text
}

fn analyze(text: &str) -> Screen {
    let count = text.chars().count();
    if count == 0 || count > MAX_CHARS || !crate::bidi::has_rtl(text) {
        return Screen { text: text.to_owned(), order: (0..count).collect(), rtl: vec![false; count], base_rtl: false };
    }
    let info = BidiInfo::new(text, None);
    let entries: Vec<(usize, usize, char)> = text.char_indices().enumerate().map(|(index, (byte, ch))| (byte, index, ch)).collect();
    let mut text_out = String::with_capacity(text.len());
    let mut order = Vec::with_capacity(count);
    let mut rtl = Vec::with_capacity(count);
    let mut claimed = vec![false; count];
    let base_rtl = info.paragraphs.first().is_some_and(|p| p.level.is_rtl());
    for para in &info.paragraphs {
        if para.range.is_empty() {
            continue;
        }
        let (levels, runs) = info.visual_runs(para, para.range.clone());
        for run in runs {
            let run_rtl = levels.get(run.start).is_some_and(Level::is_rtl);
            // Entries are in byte order, so each run is a slice, not a scan of the whole line.
            let lo = entries.partition_point(|entry| entry.0 < run.start);
            let hi = entries.partition_point(|entry| entry.0 < run.end);
            let mut items: Vec<(usize, char)> = Vec::new();
            for &(_, index, ch) in entries.get(lo..hi).unwrap_or(&[]) {
                items.push((index, ch));
                if let Some(slot) = claimed.get_mut(index) {
                    *slot = true;
                }
            }
            if run_rtl {
                items.reverse();
            }
            for (index, ch) in items {
                text_out.push(if run_rtl { crate::bidi::mirrored(ch) } else { ch });
                order.push(index);
                rtl.push(run_rtl);
            }
        }
    }
    // A character no run claimed stays in stored order, so the map is always a permutation.
    for &(_, index, ch) in &entries {
        if claimed.get(index).copied().unwrap_or(true) {
            continue;
        }
        text_out.push(ch);
        order.push(index);
        rtl.push(false);
    }
    Screen { text: text_out, order, rtl, base_rtl }
}

struct RtlEdit {
    logical: Rc<RefCell<String>>,
    screen: Screen,
    /// Screen text egui has already been shown, with the stored query at that moment.
    /// Undo restores one of these instead of writing the on-screen order into the query.
    snapshots: UndoSnapshots,
    /// Logical text removed by a cut this frame, for the clipboard.
    deleted: Option<String>,
}

impl RtlEdit {
    fn new(logical: Rc<RefCell<String>>, snapshots: UndoSnapshots) -> Self {
        let screen = analyze(&logical.borrow());
        let mut edit = Self { logical, screen, snapshots, deleted: None };
        edit.remember();
        edit
    }

    fn refresh(&mut self) {
        self.screen = analyze(&self.logical.borrow());
        self.remember();
    }

    fn remember(&mut self) {
        let logical = {
            let logical = self.logical.borrow();
            if let Some((screen, stored)) = self.snapshots.back()
                && screen == &self.screen.text
                && stored == logical.as_str()
            {
                return;
            }
            logical.clone()
        };
        let snapshots = std::sync::Arc::make_mut(&mut self.snapshots);
        snapshots.push_back((self.screen.text.clone(), logical));
        if snapshots.len() > SNAPSHOT_CAP {
            snapshots.pop_front();
        }
    }

    fn pure_rtl(&self) -> bool {
        self.screen.base_rtl && !self.screen.rtl.is_empty() && self.screen.rtl.iter().all(|&rtl| rtl)
    }

    /// Stored index where a caret before shown character `at` inserts.
    ///
    /// On a right-to-left line the left edge is the end of the stored text, and the right edge
    /// is its start. A caret just to the left of a right-to-left character is after that
    /// character in stored order.
    fn insert_index(&self, at: usize) -> usize {
        let n = self.screen.order.len();
        let at = at.min(n);
        if at == 0 {
            return if self.screen.base_rtl { n } else { 0 };
        }
        if at == n {
            return if self.screen.base_rtl { 0 } else { n };
        }
        let logical = self.screen.order.get(at).copied().unwrap_or(n);
        let rtl = self.screen.rtl.get(at).copied().unwrap_or(self.screen.base_rtl);
        if rtl { logical.saturating_add(1).min(n) } else { logical }
    }

    /// Shown caret index for a stored boundary (`0` = before the first stored character).
    fn caret_for_boundary(&self, boundary: usize) -> usize {
        let n = self.screen.order.len();
        let boundary = boundary.min(n);
        let mut hits = Vec::new();
        for at in 0..=n {
            if self.insert_index(at) == boundary {
                hits.push(at);
            }
        }
        let rtl_side = boundary > 0 && self.logical_is_rtl(boundary.saturating_sub(1));
        if rtl_side { hits.first().copied().unwrap_or(0) } else { hits.last().copied().unwrap_or(boundary) }
    }

    fn logical_is_rtl(&self, index: usize) -> bool {
        self.screen
            .order
            .iter()
            .position(|&logical| logical == index)
            .and_then(|shown| self.screen.rtl.get(shown))
            .copied()
            .unwrap_or(self.screen.base_rtl)
    }

    fn insert_at_boundary(&mut self, boundary: usize, text: &str) -> usize {
        let text = text.replace(['\r', '\n'], " ");
        if text.is_empty() {
            return 0;
        }
        let added = text.chars().count();
        {
            let mut logical = self.logical.borrow_mut();
            let boundary = boundary.min(logical.chars().count());
            let byte = logical.char_indices().nth(boundary).map(|(b, _)| b).unwrap_or(logical.len());
            // `byte` came from `char_indices`, so it is on a character boundary.
            if byte > logical.len() || !logical.is_char_boundary(byte) {
                return 0;
            }
            logical.insert_str(byte, &text);
        }
        self.refresh();
        added
    }

    fn delete_shown(&mut self, range: Range<usize>) {
        let Some(shown) = self.screen.order.get(range) else { return };
        let mut gone = shown.to_vec();
        if gone.is_empty() {
            return;
        }
        gone.sort_unstable();
        gone.dedup();
        let mut removed = String::new();
        {
            let mut logical = self.logical.borrow_mut();
            let mut kept = String::with_capacity(logical.len());
            for (index, ch) in logical.chars().enumerate() {
                if gone.binary_search(&index).is_ok() {
                    removed.push(ch);
                } else {
                    kept.push(ch);
                }
            }
            *logical = kept;
        }
        if !removed.is_empty() {
            self.deleted = Some(removed);
        }
        self.refresh();
    }

    /// Remove one stored character and leave the caret at that boundary.
    fn delete_logical(&mut self, logical_index: usize) -> CCursor {
        let Some(shown) = self.screen.order.iter().position(|&logical| logical == logical_index) else {
            return CCursor { index: CharIndex(0), prefer_next_row: true };
        };
        self.delete_shown(shown..shown.saturating_add(1));
        let boundary = logical_index.min(self.screen.order.len());
        CCursor { index: CharIndex(self.caret_for_boundary(boundary)), prefer_next_row: true }
    }

    /// Remove stored characters `start..end` and leave the caret at `start`.
    fn delete_logical_span(&mut self, start: usize, end: usize) -> CCursor {
        let count = self.logical.borrow().chars().count();
        let start = start.min(count);
        let end = end.min(count).max(start);
        if start == end {
            return CCursor { index: CharIndex(self.caret_for_boundary(start)), prefer_next_row: true };
        }
        let mut removed = String::new();
        {
            let mut logical = self.logical.borrow_mut();
            let mut kept = String::with_capacity(logical.len());
            for (index, ch) in logical.chars().enumerate() {
                if (start..end).contains(&index) {
                    removed.push(ch);
                } else {
                    kept.push(ch);
                }
            }
            *logical = kept;
        }
        if !removed.is_empty() {
            self.deleted = Some(removed);
        }
        self.refresh();
        let boundary = start.min(self.screen.order.len());
        CCursor { index: CharIndex(self.caret_for_boundary(boundary)), prefer_next_row: true }
    }

    fn logical_screen_slice(&self, range: Range<usize>) -> String {
        let Some(shown) = self.screen.order.get(range) else { return String::new() };
        let mut wanted = shown.to_vec();
        wanted.sort_unstable();
        let logical = self.logical.borrow();
        let mut out = String::new();
        for (index, ch) in logical.chars().enumerate() {
            if wanted.binary_search(&index).is_ok() {
                out.push(ch);
            }
        }
        out
    }
}

impl TextBuffer for RtlEdit {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        &self.screen.text
    }

    fn insert_text(&mut self, text: &str, char_index: CharIndex) -> usize {
        let boundary = self.insert_index(char_index.0);
        self.insert_at_boundary(boundary, text)
    }

    fn delete_char_range(&mut self, char_range: Range<CharIndex>) {
        self.delete_shown(char_range.start.0..char_range.end.0);
    }

    fn insert_text_at(&mut self, ccursor: &mut CCursor, text_to_insert: &str, char_limit: usize) {
        let room = char_limit.saturating_sub(self.logical.borrow().chars().count());
        if room == 0 || text_to_insert.is_empty() {
            return;
        }
        let end = text_to_insert.char_indices().nth(room).map(|(byte, _)| byte).unwrap_or(text_to_insert.len());
        let text = text_to_insert.get(..end).unwrap_or("");
        if text.is_empty() {
            return;
        }
        let boundary = self.insert_index(ccursor.index.0);
        let added = self.insert_at_boundary(boundary, text);
        ccursor.index = CharIndex(self.caret_for_boundary(boundary.saturating_add(added)));
    }

    fn replace_with(&mut self, text: &str) {
        if let Some((_, stored)) = self.snapshots.iter().rev().find(|(screen, _)| screen == text) {
            let stored = stored.clone();
            *self.logical.borrow_mut() = stored;
            self.refresh();
            return;
        }
        // An unknown right-to-left snapshot is the on-screen order. Storing it would make
        // search look for the reversed words.
        if !crate::bidi::has_rtl(text) {
            *self.logical.borrow_mut() = text.to_owned();
            self.refresh();
        }
    }

    fn delete_previous_char(&mut self, ccursor: CCursor) -> CCursor {
        if self.pure_rtl() {
            let boundary = self.insert_index(ccursor.index.0);
            if boundary == 0 {
                return ccursor;
            }
            return self.delete_logical(boundary - 1);
        }
        if ccursor.index.0 == 0 {
            return ccursor;
        }
        let at = ccursor.index.0;
        self.delete_shown(at.saturating_sub(1)..at);
        CCursor { index: CharIndex(at.saturating_sub(1).min(self.screen.order.len())), prefer_next_row: true }
    }

    fn delete_next_char(&mut self, ccursor: CCursor) -> CCursor {
        if self.pure_rtl() {
            let boundary = self.insert_index(ccursor.index.0);
            if boundary >= self.screen.order.len() {
                return ccursor;
            }
            return self.delete_logical(boundary);
        }
        let at = ccursor.index.0;
        if at >= self.screen.order.len() {
            return ccursor;
        }
        self.delete_shown(at..at + 1);
        CCursor { index: CharIndex(at.min(self.screen.order.len())), prefer_next_row: true }
    }

    fn delete_previous_word(&mut self, ccursor: CCursor) -> CCursor {
        if self.pure_rtl() {
            let boundary = self.insert_index(ccursor.index.0);
            let logical = self.logical.borrow().clone();
            let previous = egui::text_selection::text_cursor_state::ccursor_previous_word(&logical, CCursor::new(boundary));
            return self.delete_logical_span(previous.index.0, boundary);
        }
        let screen = self.as_str().to_owned();
        let previous = egui::text_selection::text_cursor_state::ccursor_previous_word(&screen, ccursor);
        self.delete_char_range(previous.index..ccursor.index);
        CCursor { index: previous.index, prefer_next_row: true }
    }

    fn delete_next_word(&mut self, ccursor: CCursor) -> CCursor {
        if self.pure_rtl() {
            let boundary = self.insert_index(ccursor.index.0);
            let logical = self.logical.borrow().clone();
            let next = egui::text_selection::text_cursor_state::ccursor_next_word(&logical, CCursor::new(boundary));
            return self.delete_logical_span(boundary, next.index.0);
        }
        let screen = self.as_str().to_owned();
        let next = egui::text_selection::text_cursor_state::ccursor_next_word(&screen, ccursor);
        let at = ccursor.index;
        self.delete_char_range(at..next.index);
        CCursor { index: CharIndex(at.0.min(self.screen.order.len())), prefer_next_row: true }
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::text::cursor::CCursor;

    fn edit(text: &str) -> RtlEdit {
        RtlEdit::new(Rc::new(RefCell::new(text.to_owned())), std::sync::Arc::new(std::collections::VecDeque::new()))
    }

    fn type_into(start: &str, added: &str) -> (String, usize) {
        let mut field = edit(start);
        let mut cursor = CCursor { index: CharIndex(field.caret_for_boundary(start.chars().count())), prefer_next_row: false };
        field.insert_text_at(&mut cursor, added, usize::MAX);
        (field.logical.borrow().clone(), cursor.index.0)
    }

    fn type_each(chars: &str) -> (String, usize) {
        let mut field = edit("");
        let mut cursor = CCursor::default();
        for ch in chars.chars() {
            field.insert_text_at(&mut cursor, &ch.to_string(), usize::MAX);
        }
        (field.logical.borrow().clone(), cursor.index.0)
    }

    fn assert_permutation(text: &str) {
        let screen = analyze(text);
        let mut seen = screen.order.clone();
        seen.sort_unstable();
        let expect: Vec<usize> = (0..text.chars().count()).collect();
        assert_eq!(seen, expect, "{text:?} -> {:?}", screen.text);
        assert_eq!(screen.text.chars().count(), text.chars().count());
        assert_eq!(screen.rtl.len(), text.chars().count());
    }

    #[test]
    fn latin_text_is_unchanged_and_the_caret_stays_at_the_end() {
        assert_eq!(screen_of("page"), "page");
        assert!(!analyze("page").base_rtl);
        let (text, caret) = type_each("page");
        assert_eq!((text.as_str(), caret), ("page", 4));
        let (text, caret) = type_into("pa", "ge");
        assert_eq!((text.as_str(), caret), ("page", 4));
    }

    #[test]
    fn arabic_words_type_in_logical_order_and_draw_in_reading_order() {
        // Typing order, the string search matches.
        let phrase = "مؤشرات الأداء";
        let (text, caret) = type_each(phrase);
        assert_eq!(text, phrase);
        // The leading edge of a right-to-left line is the left side.
        assert_eq!(caret, 0);
        // Shown left to right, the line is the stored characters reversed, so the first typed
        // word is on the right. "الأداء مؤشرات" read right to left.
        // Code points, not an Arabic literal: a literal in this file is reordered by the bidi
        // algorithm of the editor, so the bytes would not be the order under test.
        // ء ا د أ ل ا   ت ا ر ش ؤ م  — the stored phrase reversed, first word on the right.
        let shown: Vec<u32> = screen_of(phrase).chars().map(u32::from).collect();
        assert_eq!(shown, [0x0621, 0x0627, 0x062F, 0x0623, 0x0644, 0x0627, 0x20, 0x062A, 0x0627, 0x0631, 0x0634, 0x0624, 0x0645]);
        assert!(analyze(phrase).base_rtl);
        assert_ne!(screen_of(phrase), phrase);
    }

    #[test]
    fn arabic_backspace_removes_the_last_typed_character() {
        let mut field = edit("");
        let mut cursor = CCursor::default();
        for ch in "مؤشرات".chars() {
            field.insert_text_at(&mut cursor, &ch.to_string(), usize::MAX);
        }
        assert_eq!(cursor.index.0, 0);
        cursor = field.delete_previous_char(cursor);
        assert_eq!(field.logical.borrow().as_str(), "مؤشرا");
        assert_eq!(cursor.index.0, 0);
        // Latin backspace still removes the character before the caret.
        let mut field = edit("page");
        let cursor = CCursor { index: CharIndex(4), prefer_next_row: false };
        let cursor = field.delete_previous_char(cursor);
        assert_eq!(field.logical.borrow().as_str(), "pag");
        assert_eq!(cursor.index.0, 3);
    }

    #[test]
    fn inserting_in_the_middle_of_latin_and_at_the_edges_of_arabic() {
        let mut field = edit("ab");
        let mut cursor = CCursor { index: CharIndex(1), prefer_next_row: false };
        field.insert_text_at(&mut cursor, "X", usize::MAX);
        assert_eq!(field.logical.borrow().as_str(), "aXb");
        assert_eq!(cursor.index.0, 2);

        // Right edge of an Arabic line is the start of the stored text.
        let mut field = edit("مر");
        let at = field.screen.order.len();
        let mut cursor = CCursor { index: CharIndex(at), prefer_next_row: false };
        field.insert_text_at(&mut cursor, "ا", usize::MAX);
        assert_eq!(field.logical.borrow().as_str(), "امر");
    }

    #[test]
    fn backspace_at_the_right_edge_of_arabic_deletes_nothing() {
        let mut field = edit("مر");
        let end = CCursor { index: CharIndex(field.screen.order.len()), prefer_next_row: false };
        let cursor = field.delete_previous_char(end);
        assert_eq!(field.logical.borrow().as_str(), "مر");
        assert_eq!(cursor.index.0, end.index.0);
        // The leading edge is after the last typed letter: Backspace removes that letter.
        let cursor = field.delete_previous_char(CCursor { index: CharIndex(0), prefer_next_row: false });
        assert_eq!(field.logical.borrow().as_str(), "م");
        assert_eq!(cursor.index.0, 0);
    }

    #[test]
    fn delete_at_the_right_edge_removes_the_first_stored_letter() {
        let mut field = edit("مر");
        let end = CCursor { index: CharIndex(field.screen.order.len()), prefer_next_row: false };
        let _cursor = field.delete_next_char(end);
        assert_eq!(field.logical.borrow().as_str(), "ر");
    }

    #[test]
    fn undo_restores_the_stored_text_not_the_screen_text() {
        let mut field = edit("");
        let mut cursor = CCursor::default();
        for ch in "مر".chars() {
            field.insert_text_at(&mut cursor, &ch.to_string(), usize::MAX);
        }
        let screen = field.as_str().to_owned();
        let stored = field.logical.borrow().clone();
        assert_ne!(screen, stored);
        field.insert_text_at(&mut cursor, "ا", usize::MAX);
        field.replace_with(&screen);
        assert_eq!(field.logical.borrow().as_str(), stored);
        // A reversed string egui did not snapshot must not become the query.
        field.replace_with(&screen.chars().rev().collect::<String>());
        assert_eq!(field.logical.borrow().as_str(), stored);
    }

    /// egui records one undo point for a whole burst of keystrokes. The screen string from
    /// before that burst has to still be in the map after more edits than the old cap of 100.
    #[test]
    fn a_long_burst_still_undoes_to_the_phrase_typed_before_it() {
        let mut field = edit("");
        let mut cursor = CCursor::default();
        for ch in "مر".chars() {
            field.insert_text_at(&mut cursor, &ch.to_string(), usize::MAX);
        }
        let screen = field.as_str().to_owned();
        let stored = field.logical.borrow().clone();
        for _ in 0..150 {
            field.insert_text_at(&mut cursor, "ا", usize::MAX);
        }
        assert!(field.logical.borrow().chars().count() > stored.chars().count());
        field.replace_with(&screen);
        assert_eq!(field.logical.borrow().as_str(), stored, "undo lost the phrase from before the burst");
    }

    /// An unchanged frame must not copy the history. egui clones temp data on every read,
    /// so the history stays behind one `Arc` until an edit actually changes it.
    #[test]
    fn an_unchanged_refresh_keeps_the_undo_allocation() {
        let mut field = edit("page");
        let mut cursor = CCursor::default();
        for i in 0..40 {
            field.insert_text_at(&mut cursor, &format!("{i:02}"), usize::MAX);
        }
        let before = std::sync::Arc::as_ptr(&field.snapshots);
        let retained: usize = field.snapshots.iter().map(|(screen, stored)| screen.len().saturating_add(stored.len())).sum();
        assert!(retained > 1_000, "the history is too small to show a copy: {retained}");
        field.refresh();
        assert_eq!(std::sync::Arc::as_ptr(&field.snapshots), before, "refresh copied the undo history");
        let screen = field.as_str().to_owned();
        let stored = field.logical.borrow().clone();
        field.replace_with(&screen);
        assert_eq!(field.logical.borrow().as_str(), stored);
    }

    /// Two frames with the same query share one history allocation, for both fields.
    #[test]
    fn two_frames_share_one_undo_history() {
        let ctx = egui::Context::default();
        let mut query = String::from("page");
        let mut seen = Vec::new();
        for _ in 0..2 {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show_query(ui, &mut query, QueryField::FindBar);
                show_query(ui, &mut query, QueryField::SearchPanel);
                let ptr = ui
                    .data(|d| d.get_temp::<UndoSnapshots>(egui::Id::new("find-input").with("rtl-undo")))
                    .map(|history| std::sync::Arc::as_ptr(&history) as usize)
                    .unwrap_or(0);
                seen.push(ptr);
            });
            output.drop_without_applying_deltas();
        }
        assert_eq!(seen.len(), 2);
        assert_ne!(seen[0], 0, "the find field stored no undo history");
        assert_eq!(seen[0], seen[1], "the second frame copied the undo history");
        assert_eq!(query, "page");
    }

    /// Backspace, Delete, and an undo of a blank field must not invent text. An Arabic string
    /// egui did not snapshot must not become the query either.
    #[test]
    fn an_empty_field_ignores_backspace_delete_and_a_reversed_undo() {
        let mut field = edit("");
        let cursor = CCursor::default();
        assert_eq!(field.delete_previous_char(cursor).index.0, 0);
        assert_eq!(field.delete_next_char(cursor).index.0, 0);
        assert_eq!(field.logical.borrow().as_str(), "");
        field.replace_with("");
        assert_eq!(field.logical.borrow().as_str(), "");
        field.replace_with("مر");
        assert_eq!(field.logical.borrow().as_str(), "", "an unknown Arabic snapshot became the query");
    }

    /// Undo then redo walks the stored texts, not the on-screen order.
    #[test]
    fn redo_puts_back_the_later_typed_text() {
        let mut field = edit("");
        let mut cursor = CCursor::default();
        for ch in "مر".chars() {
            field.insert_text_at(&mut cursor, &ch.to_string(), usize::MAX);
        }
        let early_screen = field.as_str().to_owned();
        let early = field.logical.borrow().clone();
        field.insert_text_at(&mut cursor, "ا", usize::MAX);
        let later_screen = field.as_str().to_owned();
        let later = field.logical.borrow().clone();
        assert_ne!(early_screen, early);
        field.replace_with(&early_screen);
        assert_eq!(field.logical.borrow().as_str(), early);
        field.replace_with(&later_screen);
        assert_eq!(field.logical.borrow().as_str(), later, "redo stored the on-screen order");
    }

    /// The bar and the Search panel share the query string and keep separate undo maps.
    #[test]
    fn the_find_bar_and_the_search_panel_do_not_share_undo_history() {
        let ctx = egui::Context::default();
        let mut find = String::from("page");
        let mut search = String::from("other");
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            show_query(ui, &mut find, QueryField::FindBar);
            show_query(ui, &mut search, QueryField::SearchPanel);
        });
        output.drop_without_applying_deltas();
        find.push('X');
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            show_query(ui, &mut find, QueryField::FindBar);
            show_query(ui, &mut search, QueryField::SearchPanel);
        });
        output.drop_without_applying_deltas();
        let mut found = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let find_history = ui.data(|d| d.get_temp::<UndoSnapshots>(egui::Id::new("find-input").with("rtl-undo")));
            let search_history = ui.data(|d| d.get_temp::<UndoSnapshots>(egui::Id::new("search-panel-input").with("rtl-undo")));
            found = Some((find_history, search_history));
        });
        output.drop_without_applying_deltas();
        let (find_history, search_history) = found.expect("histories");
        let find_history = find_history.expect("find history");
        let search_history = search_history.expect("search history");
        assert!(!std::sync::Arc::ptr_eq(&find_history, &search_history));
        assert!(find_history.iter().any(|(_, stored)| stored == "pageX"), "find undo lost the edit");
        assert!(search_history.iter().all(|(_, stored)| stored == "other"), "search undo took the find bar's text");
    }

    /// Past the cap, the oldest screen is gone and is not written into the query. A later
    /// screen from the same burst still restores the typed text.
    #[test]
    fn the_undo_cap_drops_only_the_oldest_snapshot() {
        let mut field = edit("");
        let mut cursor = CCursor::default();
        field.insert_text_at(&mut cursor, "م", usize::MAX);
        let dropped_screen = field.as_str().to_owned();
        // The empty field is the oldest entry. One full cap of later edits drops that and `م`.
        for _ in 0..SNAPSHOT_CAP {
            field.insert_text_at(&mut cursor, "ا", usize::MAX);
        }
        let kept_screen = field.as_str().to_owned();
        let kept_stored = field.logical.borrow().clone();
        field.insert_text_at(&mut cursor, "ا", usize::MAX);
        assert_eq!(field.snapshots.len(), SNAPSHOT_CAP);
        let current = field.logical.borrow().clone();
        field.replace_with(&dropped_screen);
        assert_eq!(field.logical.borrow().as_str(), current, "a dropped snapshot was applied");
        field.replace_with(&kept_screen);
        assert_eq!(field.logical.borrow().as_str(), kept_stored, "a snapshot inside the cap was lost");
    }

    /// A mixed line is not pure right-to-left, so word delete follows the shown word.
    /// Using the stored edge here would remove "See" and leave the Arabic word.
    #[test]
    fn a_mixed_line_word_delete_removes_the_shown_word() {
        let mut field = edit("See واحد");
        let end = field.as_str().chars().count();
        field.delete_previous_word(CCursor { index: CharIndex(end), prefer_next_row: false });
        let left = field.logical.borrow().clone();
        assert!(left.contains("See"), "word delete used the stored edge: {left:?}");
        assert!(!left.contains('و'), "the shown Arabic word stayed: {left:?}");
    }

    /// Alt+Backspace deletes the previous word in typing order. On a pure right-to-left line
    /// that word is on the left of the caret, not the left of the screen string.
    #[test]
    fn word_backspace_removes_the_last_typed_word() {
        let phrase = "مر حب";
        let mut field = edit(phrase);
        let end = phrase.chars().count();
        let previous = egui::text_selection::text_cursor_state::ccursor_previous_word(phrase, CCursor::new(end));
        let kept: String = phrase.chars().take(previous.index.0).collect();
        let cursor = field.delete_previous_word(CCursor { index: CharIndex(0), prefer_next_row: false });
        assert_eq!(field.logical.borrow().as_str(), kept, "word backspace must follow stored order");
        assert_eq!(cursor.index.0, field.caret_for_boundary(previous.index.0));
        // The right edge is the start of the stored text: nothing comes before it.
        let right = field.screen.order.len();
        let cursor = field.delete_previous_word(CCursor { index: CharIndex(right), prefer_next_row: false });
        assert_eq!(field.logical.borrow().as_str(), kept);
        assert_eq!(cursor.index.0, right.min(field.screen.order.len()));
    }

    #[test]
    fn word_delete_at_the_right_edge_removes_the_first_stored_word() {
        let phrase = "مر حب";
        let mut field = edit(phrase);
        let next = egui::text_selection::text_cursor_state::ccursor_next_word(phrase, CCursor::new(0));
        let kept: String = phrase.chars().skip(next.index.0).collect();
        let right = field.screen.order.len();
        field.delete_next_word(CCursor { index: CharIndex(right), prefer_next_row: false });
        assert_eq!(field.logical.borrow().as_str(), kept, "delete at the right edge removes the first stored word");
    }

    #[test]
    fn a_trailing_space_and_a_vowel_stay_in_the_field() {
        let (text, _) = type_each("مر ");
        assert_eq!(text, "مر ");
        assert_eq!(screen_of("مر ").chars().count(), 3);
        assert_eq!(screen_of("بَاب").chars().count(), "بَاب".chars().count());
        assert_eq!(crate::bidi::for_shaper(&screen_of("مر ")).chars().count(), 3);
    }

    #[test]
    fn a_selection_copies_and_deletes_stored_order() {
        let mut field = edit("مؤشرات");
        let slice = field.logical_screen_slice(0..field.screen.order.len());
        assert_eq!(slice, "مؤشرات");
        field.delete_shown(0..2);
        // The two shown characters on the left are the last two stored characters.
        assert_eq!(field.logical.borrow().as_str(), "مؤشر");
        assert_eq!(field.deleted.as_deref(), Some("ات"));
    }

    #[test]
    fn hebrew_and_mixed_lines_keep_a_permutation() {
        assert_eq!(screen_of("שלום עולם"), "םלוע םולש");
        assert_eq!(screen_of("See واحد"), "See دحاو");
        assert_eq!(screen_of("مرحبا (عالم)"), "(ملاع) ابحرم");
        for text in ["شלום", "A ب", "ب A", "12 مؤشرات", "واحد\nاثنين", "بَاب", ""] {
            assert_permutation(text);
        }
    }

    #[test]
    fn odd_input_stays_a_permutation_and_a_huge_paste_is_not_reordered() {
        for text in ["\u{202E}مرحبا", "\u{064B}", "(((مرحبا", " ", "\0ا", "ا\u{200D}ب", "\n\nب"] {
            assert_permutation(text);
            let mut field = edit(text);
            field.insert_text_at(&mut CCursor::default(), "ا", usize::MAX);
            assert!(field.logical.borrow().chars().count() >= text.chars().count());
        }
        let long = "ا ".repeat(60_000);
        assert_eq!(screen_of(&long), long, "past the cap the field does not build a bidi map");
        assert_permutation(&"مر ".repeat(500));
    }

    #[test]
    fn char_limit_stops_the_field_growing() {
        let mut field = edit("مر");
        field.insert_text_at(&mut CCursor::default(), "حب", 2);
        assert_eq!(field.logical.borrow().as_str(), "مر");
        field.insert_text_at(&mut CCursor::default(), "حب", 3);
        assert_eq!(field.logical.borrow().chars().count(), 3);
    }

    /// Glyphs and caret x positions for `text`, using the same layout the field paints.
    fn painted(text: &str) -> Option<(String, Vec<f32>, Vec<f32>, usize)> {
        if pdfcraft_fonts::ui_arabic_fonts().is_empty() {
            return None;
        }
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut got = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = layout_galley(ui, text, f32::INFINITY);
            let mut chars = String::new();
            let mut xs = Vec::new();
            if let Some(row) = galley.rows.first() {
                for glyph in &row.glyphs {
                    if glyph.advance_width > 0.0 {
                        chars.push(glyph.chr);
                        xs.push(glyph.pos.x);
                    }
                }
            }
            let carets: Vec<f32> =
                (0..=text.chars().count()).map(|i| galley.pos_from_cursor(CCursor { index: CharIndex(i), prefer_next_row: false }).left()).collect();
            got = Some((chars, xs, carets, galley.num_vertices));
        });
        output.drop_without_applying_deltas();
        got
    }

    #[test]
    fn the_painted_line_reads_right_to_left_and_the_caret_reaches_every_word() {
        let phrase = "مؤشرات الأداء";
        let Some((chars, xs, carets, vertices)) = painted(phrase) else { return };
        assert!(vertices > 0, "the line was tessellated");
        assert_eq!(chars, screen_of(phrase));
        // Reading order, locked by code point: the last typed word starts at the left (hamza),
        // and the first typed word ends at the right (meem). A swapped line fails this.
        let painted_chars: Vec<char> = chars.chars().collect();
        assert_eq!(painted_chars.first().copied(), Some('\u{0621}'), "left edge {chars:?}");
        assert_eq!(painted_chars.last().copied(), Some('\u{0645}'), "right edge {chars:?}");
        let space = painted_chars.iter().position(|ch| *ch == ' ');
        assert!(space.is_some_and(|i| i > 0 && i + 1 < painted_chars.len()), "the space sits between the words: {chars:?}");
        // Glyphs sit left to right.
        for pair in xs.windows(2) {
            assert!(pair[0] <= pair[1] + 0.1, "{xs:?}");
        }
        // The reported freeze: caret slots for the second word shared one x. They must not.
        let mut rounded: Vec<i32> = carets.iter().map(|x| (x * 2.0).round() as i32).collect();
        let total = rounded.len();
        rounded.sort_unstable();
        rounded.dedup();
        assert!(rounded.len() * 2 > total, "caret x positions collapsed: {carets:?}");
        // The leading edge is the left of the line, next to the last typed letter.
        let left = xs.iter().copied().fold(f32::MAX, f32::min);
        assert!((carets[0] - left).abs() < 1.0, "caret {carets:?} glyphs {xs:?}");
        // The first typed word is the right-hand side, not under the caret.
        let right = xs.iter().copied().fold(f32::MIN, f32::max);
        assert!(right > left + 20.0, "glyphs did not spread: {xs:?}");
    }

    #[test]
    fn a_trailing_space_and_a_vowel_still_get_a_caret_slot() {
        if pdfcraft_fonts::ui_arabic_fonts().is_empty() {
            return;
        }
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for text in ["مر ", "بَاب", "مر\u{200F}", "ب\u{200D}اب", "ب\u{200C}اب", "בּאב"] {
                let galley = layout_galley(ui, text, f32::INFINITY);
                let glyphs = galley.rows.first().map(|row| row.glyphs.len()).unwrap_or(0);
                let dump: Vec<String> = galley
                    .rows
                    .first()
                    .map(|row| row.glyphs.iter().map(|g| format!("{:?}/{:.1}", g.chr, g.advance_width)).collect())
                    .unwrap_or_default();
                assert_eq!(glyphs, text.chars().count(), "{text:?} froze the caret: {glyphs} glyphs {dump:?}");
                let xs: Vec<i32> = (0..=text.chars().count())
                    .map(|i| (galley.pos_from_cursor(CCursor { index: CharIndex(i), prefer_next_row: false }).left() * 2.0).round() as i32)
                    .collect();
                let mut unique = xs.clone();
                unique.sort_unstable();
                unique.dedup();
                assert!(unique.len() > 2, "caret x collapsed for {text:?}: {xs:?}");
            }
        });
        output.drop_without_applying_deltas();
    }
}
