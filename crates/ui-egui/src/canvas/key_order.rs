//! One pass gives the pages' keys to whoever holds them.
//!
//! Each page module read its keys for itself, wherever its code happened to run: some before
//! anything was drawn (`app/keys.rs`), some after the pages. So one press could do several things
//! at once: one Escape with Find open and a comment selected closed Find and deselected the comment.
//! This pass reads the pages' keys for the active view once a pass, in `document_area`: after the
//! panels have had theirs, and before the pages are drawn, while last pass's page positions are
//! still there for the nudges.
//!
//! Each key has one function below, which reads as its order: the first holder that holds
//! something takes the key, and nothing after it sees it. What reads the key itself later in the
//! pass (an editor open on the page, a press the pages hold, the Measure tool's points) is asked by
//! its state, not by egui's focus: egui drops the focus at the start of a pass with a plain Escape
//! in it, so "is a text field typing?" is false on the very pass it matters.
//!
//! Between kinds of selection, the newest goes first ([`SelectionAges`]).

use std::hash::{DefaultHasher, Hash, Hasher};

use pdfcraft_engine::{Document, Edit};

use super::DocView;
use crate::QuickTool;
use crate::page_keys::PageKeys;
use crate::scroll_speed::ScrollSpeed;

/// What the pass needs besides the view.
pub(crate) struct KeyCx<'a> {
    pub(crate) ctx: &'a egui::Context,
    pub(crate) keys: PageKeys,
    pub(crate) doc: &'a Document,
    /// The quick tool.
    pub(crate) tool: QuickTool,
    /// Prepare a form is open: fields are edited, not filled.
    pub(crate) preparing: bool,
    /// Edit a PDF: added text and images can be selected.
    pub(crate) editing_content: bool,
    pub(crate) speed: ScrollSpeed,
}

/// The pages' keys for `view`, the active one. `tool` is the quick tool, which Escape can drop.
pub(super) fn run(view: &mut DocView, cx: &KeyCx<'_>, tool: &mut QuickTool) {
    let KeyCx { ctx, keys, doc, preparing, speed, .. } = *cx;
    // The page grid has its own Escape and Delete, and shows no selection of the pages' to act on:
    // only the view's keys, which move its current page.
    if view.organize {
        super::navigation::view_plain_keys(view, ctx, keys, speed);
        return;
    }
    let now = holding(view);
    view.key_ages.note(now);
    escape(view, cx, tool);
    delete(view, cx);
    if *tool == QuickTool::EditText && doc.allows_modification() {
        crate::object_ui::nudge_keys(ctx, keys, view, doc);
    } else if doc.allows_annotation() {
        crate::comments::nudge_keys(ctx, keys, view, &doc.info);
    }
    // A selected field moves with the arrows too (Prepare a form, #2608), before the view turns
    // the page with them.
    if preparing && doc.allows_modification() {
        crate::prepare::nudge_keys(ctx, keys, view, doc);
    }
    super::navigation::view_plain_keys(view, ctx, keys, speed);
}

/// Escape: one step back per press. The first in this order that holds something takes it:
/// 1. what reads it itself later in the pass: an editor open on the page, a press the pages hold
///    (`cancel_on_escape`), the Measure tool's points;
/// 2. a comment being drawn or placed, Prepare's area selection;
/// 3. a picture's crop mode, the camera;
/// 4. a selection, the newest first;
/// 5. a comment or Fill & Sign tool, which goes back to Select;
/// 6. the Find bar.
///
/// Full screen's own Escape stays in `app/keys.rs`, ahead of these, for now (#5325).
fn escape(view: &mut DocView, cx: &KeyCx<'_>, tool: &mut QuickTool) {
    let KeyCx { ctx, keys, .. } = *cx;
    // A focused widget that keeps Escape (egui drops any other's focus on its pass) has it.
    if !keys.pressed(ctx, egui::Key::Escape) || typing(view, ctx) {
        return;
    }
    if editor_open(view) || press_held(view, ctx) || measuring(view, cx.tool) {
        return;
    }
    let taken = cancel_drawing(view) || leave_mode(view, cx) || deselect(view, cx) || drop_tool(tool) || view.find.take().is_some();
    if taken {
        keys.take(ctx, egui::Key::Escape);
    }
}

/// Delete and Backspace: one selection goes per press. The first in this order that holds something
/// takes them:
/// 1. what reads them itself later in the pass: an editor open on the page, a comment being drawn
///    point by point (its last point goes), the Measure tool's points (Backspace);
/// 2. a selection, the newest first. One that can't go yet (a button is down, an edit waits) still
///    takes the key, so an older one doesn't go in its place.
///
/// In full screen Backspace steps back a page after these (the view's keys). A text field keeps
/// them, but a focused button doesn't: after a click on the toolbar Delete still deletes (#5012).
fn delete(view: &mut DocView, cx: &KeyCx<'_>) {
    use egui::Key;
    let KeyCx { ctx, keys, doc, tool, .. } = *cx;
    let backspace = keys.pressed(ctx, Key::Backspace);
    if !(backspace || keys.pressed(ctx, Key::Delete)) || ctx.text_edit_focused() || editor_open(view) {
        return;
    }
    let drawing = matches!(view.comments.gesture, Some(crate::comments::Gesture::Draw { .. }));
    if drawing || backspace && matches!(tool, QuickTool::Measure(_)) && !view.measure.points.is_empty() {
        return;
    }
    let edit_text = tool == QuickTool::EditText && doc.allows_modification();
    let held = ctx.input(|i| i.pointer.any_down());
    for kind in view.key_ages.newest_first() {
        let took = match kind {
            Held::Objects => edit_text && crate::object_ui::delete_key(view),
            Held::Picture => edit_text && crate::edit_text_ui::delete_picture(view, doc, held),
            Held::Added => cx.editing_content && crate::content_ui::delete_key(view, tool == QuickTool::EditText),
            Held::Comment => crate::comments::delete_key(view, &doc.info, doc.allows_annotation()),
            Held::Field => cx.preparing && crate::prepare::delete_key(view),
            Held::Link => tool == QuickTool::Link && crate::link_ui::delete_key(view),
            Held::Picked | Held::Text => false,
        };
        if took {
            keys.take(ctx, Key::Delete);
            keys.take(ctx, Key::Backspace);
            return;
        }
    }
}

/// A text field (or another widget that keeps the plain keys) has the keyboard. A focused check box
/// or radio button of a form holds it only for Space (and a radio button for the arrows).
fn typing(view: &DocView, ctx: &egui::Context) -> bool {
    let form_box = view.forms.keys.is_some_and(|(id, _)| ctx.memory(|m| m.focused()) == Some(id));
    ctx.egui_wants_keyboard_input() && !form_box
}

/// An editor is open on the page: a paragraph, added text, a comment being written, a form field
/// being filled or a Fill & Sign text box. It reads its own keys.
fn editor_open(view: &DocView) -> bool {
    view.line_editor.is_some()
        || view.content.draft.is_some()
        || view.comments.composer.is_some()
        || view.forms.focus.is_some()
        || view.fill_text.is_some()
}

/// The pages held a press last pass and the button is still down: `cancel_on_escape` cancels it
/// this pass.
fn press_held(view: &DocView, ctx: &egui::Context) -> bool {
    view.held_press && ctx.input(|i| i.pointer.primary_down())
}

/// The Measure tool has points down or a reading: it cancels them itself.
fn measuring(view: &DocView, tool: QuickTool) -> bool {
    matches!(tool, QuickTool::Measure(_)) && (!view.measure.points.is_empty() || view.measure.reading.is_some())
}

/// A comment being drawn or placed, or Prepare's area selection, is abandoned.
fn cancel_drawing(view: &mut DocView) -> bool {
    if view.comments.gesture.take().is_some() {
        return true;
    }
    crate::prepare::escape_marquee(&mut view.prepare)
}

/// A picture's crop mode is left (not while a button is down: Escape then cancels the press), and
/// the camera is put away.
fn leave_mode(view: &mut DocView, cx: &KeyCx<'_>) -> bool {
    let down = cx.ctx.input(|i| i.pointer.any_down());
    if !down && let Some(s) = view.image_selection.as_mut().filter(|s| s.crop.is_some()) {
        s.crop = None;
        return true;
    }
    if view.camera.is_some() || view.camera_done.is_some() {
        view.pending_action = Some(super::ViewAction::CancelCamera);
        return true;
    }
    false
}

/// The newest selection is let go. The picture's isn't: Escape leaves only its crop mode.
fn deselect(view: &mut DocView, cx: &KeyCx<'_>) -> bool {
    for held in view.key_ages.newest_first() {
        let done = match held {
            Held::Objects => view.objects.deselect(),
            Held::Picture => false,
            Held::Added => {
                let picture =
                    cx.tool == QuickTool::EditText && view.image_selection.is_some() || matches!(view.pending_edit, Some(Edit::EditPageImage { .. }));
                cx.editing_content && !picture && view.content.selected.take().is_some()
            }
            Held::Comment => {
                let any = view.comments.selected.is_some() || !view.comments.also.is_empty();
                view.comments.clear_selection();
                any
            }
            Held::Field => {
                let any = cx.preparing && view.prepare.selected.is_some();
                if any {
                    view.prepare.selected = None;
                    view.prepare.also.clear();
                }
                any
            }
            Held::Link => cx.tool == QuickTool::Link && view.links.selected.take().is_some(),
            Held::Picked => {
                let any = view.picked_image.is_some() || view.pending_pick.is_some();
                view.picked_image = None;
                view.pending_pick = None;
                any
            }
            Held::Text => {
                let any = view.selection.is_some() || view.all_text;
                view.selection = None;
                view.all_text = false;
                any
            }
        };
        if done {
            return true;
        }
    }
    false
}

/// A comment tool or a Fill & Sign one goes back to Select: its Text tool would otherwise open a
/// box at every click with no way out by the keyboard.
fn drop_tool(tool: &mut QuickTool) -> bool {
    let drops = matches!(tool, QuickTool::Comment(_) | QuickTool::Fill(_));
    if drops {
        *tool = QuickTool::Select;
    }
    drops
}

/// A kind of selection on the pages, in the order the pass asks them when two were made in the same
/// pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    Objects,
    Picture,
    Added,
    Comment,
    Field,
    Link,
    Picked,
    Text,
}

const KINDS: usize = 8;
const ALL: [Held; KINDS] = [Held::Objects, Held::Picture, Held::Added, Held::Comment, Held::Field, Held::Link, Held::Picked, Held::Text];

/// What each kind of selection holds now, as a fingerprint, or `None`.
fn holding(view: &DocView) -> [Option<u64>; KINDS] {
    fn print(value: impl Hash) -> u64 {
        let mut h = DefaultHasher::new();
        value.hash(&mut h);
        h.finish()
    }
    let comments = &view.comments;
    let text = view.selection.map(|s| {
        let span = match s.span {
            super::selection::Span::Run { anchor, head } => [anchor as u64, head as u64, 0, 0],
            super::selection::Span::Column { rect } => rect.map(|v| u64::from(v.to_bits())),
        };
        (s.page, span)
    });
    [
        view.objects.selected_targets().map(print),
        view.image_selection.as_ref().map(|s| print((s.page, s.index))),
        view.content.selected.map(print),
        (comments.selected.is_some() || !comments.also.is_empty()).then(|| print((comments.selected, &comments.also))),
        view.prepare.selected.as_ref().map(|s| print((s, &view.prepare.also))),
        view.links.selected.map(print),
        view.picked_image.map(|p| print((p.page, p.index))).or(view.pending_pick.map(|(page, _, n)| print((page, n)))),
        (text.is_some() || view.all_text).then(|| print((text, view.all_text))),
    ]
}

/// When each kind of selection was last made or changed, so that the newest takes a key first:
/// select some text, then a comment, and Escape lets go of the comment, then the text.
#[derive(Clone, Debug, Default)]
pub(crate) struct SelectionAges {
    seen: [Option<u64>; KINDS],
    made: [u64; KINDS],
    clock: u64,
}

impl SelectionAges {
    /// What each kind holds now: a kind holding something it didn't hold last time is the newest.
    fn note(&mut self, now: [Option<u64>; KINDS]) {
        let tick = self.clock.saturating_add(1);
        let mut changed = false;
        for (made, (seen, now)) in self.made.iter_mut().zip(self.seen.iter().zip(now)) {
            if now.is_some() && now != *seen {
                *made = tick;
                changed = true;
            }
        }
        if changed {
            self.clock = tick;
        }
        self.seen = now;
    }

    /// Every kind, the newest first; kinds made in the same pass in the fixed order.
    fn newest_first(&self) -> [Held; KINDS] {
        let mut order = ALL;
        order.sort_by_key(|h| std::cmp::Reverse(self.made.get(*h as usize).copied().unwrap_or(0)));
        order
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PdfCraftApp;

    /// Escape takes back a click whose picture isn't known yet: it isn't selected when it is.
    #[test]
    fn escape_cancels_a_click_on_a_page_still_being_read() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("notes.txt", None, b"hello".to_vec()).unwrap();
        let (i, id) = app.active_ids().unwrap();
        app.views[i].pending_pick = Some((0, [1.0, 1.0], 0));
        let escape = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let Some(doc) = app.session.get(id) else { panic!("the document is open") };
        let view = &mut app.views[i];
        egui::Context::default()
            .run_ui(egui::RawInput { events: vec![escape], ..Default::default() }, |ui| {
                let cx = KeyCx {
                    ctx: ui.ctx(),
                    keys: PageKeys::new(false),
                    doc,
                    tool: QuickTool::Select,
                    preparing: false,
                    editing_content: false,
                    speed: ScrollSpeed::default(),
                };
                run(view, &cx, &mut QuickTool::Select);
            })
            .drop_without_applying_deltas();
        assert_eq!(app.views[i].pending_pick, None);
    }

    #[test]
    fn the_newest_selection_goes_first_and_ties_keep_the_fixed_order() {
        let mut ages = SelectionAges::default();
        assert_eq!(ages.newest_first(), ALL, "nothing made yet: the fixed order");
        let mut now = [None; KINDS];
        now[Held::Text as usize] = Some(1);
        ages.note(now);
        now[Held::Comment as usize] = Some(2);
        ages.note(now);
        assert_eq!(ages.newest_first()[..2], [Held::Comment, Held::Text], "the comment was selected after the text");
        // Another bit of text selected: the text is the newest again.
        now[Held::Text as usize] = Some(3);
        ages.note(now);
        assert_eq!(ages.newest_first()[..2], [Held::Text, Held::Comment]);
        // Unchanged: no new order.
        ages.note(now);
        assert_eq!(ages.newest_first()[..2], [Held::Text, Held::Comment]);
        // Two made in the same pass keep the fixed order between them.
        let mut ages = SelectionAges::default();
        let mut both = [None; KINDS];
        both[Held::Link as usize] = Some(1);
        both[Held::Objects as usize] = Some(1);
        ages.note(both);
        assert_eq!(ages.newest_first()[..2], [Held::Objects, Held::Link]);
    }

    #[test]
    fn a_selection_let_go_and_made_again_is_new_again() {
        let mut ages = SelectionAges::default();
        let mut now = [None; KINDS];
        now[Held::Comment as usize] = Some(7);
        ages.note(now);
        now[Held::Text as usize] = Some(1);
        ages.note(now);
        now[Held::Comment as usize] = None;
        ages.note(now);
        now[Held::Comment as usize] = Some(7);
        ages.note(now);
        assert_eq!(ages.newest_first()[0], Held::Comment, "the same comment selected again is the newest");
    }
}
