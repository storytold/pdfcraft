//! Keep composition confirmation/cancellation keys out of document commands.
//!
//! Text stays logical and egui owns shaping, grapheme motion and the preedit range.
//! This remembers only which focused widget has an unfinished composition.

use egui::{Context, Event, Id, ImeEvent, Key};

#[derive(Clone, Copy, Default)]
struct Composition {
    owner: Option<Id>,
    active: bool,
    blocked_primary: bool,
}

impl Composition {
    fn observe(&mut self, owner: Option<Id>, events: &[Event]) -> bool {
        if self.owner != owner || owner.is_none() {
            self.owner = owner;
            self.active = false;
        }
        if owner.is_none() {
            return false;
        }
        let mut owns_keys = self.active;
        for event in events {
            match event {
                Event::Ime(ImeEvent::Preedit { text, .. }) => {
                    owns_keys |= self.active || !text.is_empty();
                    self.active = !text.is_empty();
                }
                Event::Ime(ImeEvent::Commit(text)) => {
                    owns_keys |= self.active || !text.is_empty();
                    self.active = false;
                }
                Event::WindowFocused(false) => {
                    self.active = false;
                    self.blocked_primary = false;
                }
                // Enabled/Disabled are obsolete in egui; empty idle preedit is not
                // an active composition. Do not guess state from either of those.
                _ => {}
            }
        }
        owns_keys
    }
}

fn state_id() -> Id {
    Id::unique("pdfcraft-input-composition")
}

pub(crate) fn composing_for(ctx: &Context, owner: Id) -> bool {
    ctx.data(|d| d.get_temp::<Composition>(state_id())).is_some_and(|s| s.owner == Some(owner) && s.active)
}

pub(crate) fn is_composing(ctx: &Context) -> bool {
    ctx.memory(|m| m.focused()).is_some_and(|owner| composing_for(ctx, owner))
}

pub(crate) fn forget(ctx: &Context, owner: Id) {
    if composing_for(ctx, owner) {
        ctx.data_mut(|d| d.remove::<Composition>(state_id()));
    }
}

fn command_key(event: &Event) -> bool {
    matches!(event, Event::Key { key: Key::Enter | Key::Escape | Key::Tab, pressed: true, .. })
}

fn focused_editor(ctx: &Context) -> Option<Id> {
    if ctx.text_edit_focused() { ctx.memory(|m| m.focused()) } else { None }
}

/// Before egui interprets Tab, and before any document shortcuts read the keys.
pub(crate) fn protect_raw(ctx: &Context, events: &mut Vec<Event>, inline: Option<(Id, egui::Rect)>) {
    let owner = focused_editor(ctx);
    let mut composition = ctx.data(|d| d.get_temp::<Composition>(state_id())).unwrap_or_default();
    let owns_keys = composition.observe(owner, events);
    // Filter a document click before egui can move native IME ownership. App
    // controls remain available, as do caret clicks inside the current editor.
    let paragraph = inline.filter(|(id, _)| owner == Some(*id)).and_then(|(id, document)| ctx.read_response(id).map(|r| (r.rect, document)));
    events.retain(|event| match event {
        Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. }
            if owns_keys && paragraph.is_some_and(|(editor, document)| document.contains(*pos) && !editor.contains(*pos)) =>
        {
            composition.blocked_primary = true;
            false
        }
        Event::PointerButton { button: egui::PointerButton::Primary, pressed: false, .. } if composition.blocked_primary => {
            // Also consume its release after an IME commit/cancel or owner change,
            // so an ignored press cannot become a later click or drag.
            composition.blocked_primary = false;
            false
        }
        _ => !owns_keys || !command_key(event),
    });
    ctx.data_mut(|d| d.insert_temp(state_id(), composition));
}

/// Hosts that do not call eframe's raw-input hook still protect application keys.
pub(crate) fn protect_current(ctx: &Context) {
    let owner = focused_editor(ctx);
    let mut composition = ctx.data(|d| d.get_temp::<Composition>(state_id())).unwrap_or_default();
    let owns_keys = ctx.input(|i| composition.observe(owner, &i.events));
    ctx.data_mut(|d| d.insert_temp(state_id(), composition));
    if owns_keys {
        ctx.input_mut(|i| i.events.retain(|event| !command_key(event)));
    }
}

/// Release composition ownership after native editors accept a pointer interruption.
pub(crate) fn finish_frame(ctx: &Context) {
    let Some(owner) = focused_editor(ctx) else { return };
    if ctx.interaction_snapshot(|interaction| interaction.clicked == Some(owner) || interaction.dragged == Some(owner)) {
        forget(ctx, owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preedit(text: &str) -> Event {
        Event::Ime(ImeEvent::Preedit { text: text.into(), active_range_chars: None })
    }

    #[test]
    fn ime_keys_stay_owned_through_idle_commit_and_cancel_frames() {
        let owner = Some(Id::unique("editor"));
        for ending in [preedit(""), Event::Ime(ImeEvent::Commit("مرحبا".into()))] {
            let mut state = Composition::default();
            assert!(state.observe(owner, &[preedit("مر")]));
            assert!(state.observe(owner, &[]), "idle frames do not finish a composition");
            assert!(state.observe(owner, &[ending]), "the finishing key belongs to the IME");
            assert!(!state.observe(owner, &[]), "ordinary commands resume next frame");
        }
    }

    #[test]
    fn idle_empty_preedit_and_other_widget_do_not_capture_commands() {
        let first = Some(Id::unique("first"));
        let second = Some(Id::unique("second"));
        let mut state = Composition::default();
        assert!(!state.observe(first, &[preedit("")]));
        assert!(state.observe(first, &[preedit("ع")]));
        assert!(!state.observe(second, &[]), "composition state cannot follow focus");
        assert!(!state.observe(None, &[preedit("ع")]));
    }

    #[test]
    fn raw_ime_routing_preserves_payload_and_regular_editing_keys() {
        let ctx = Context::default();
        let owner = Id::unique("editor");
        let mut value = String::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::TextEdit::singleline(&mut value).id(owner).show(ui).response.request_focus();
            });
            output.textures_delta.clear();
        }
        let key = |key| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let mut events = vec![preedit("لاَ"), key(Key::Enter), key(Key::Escape), key(Key::Tab), key(Key::ArrowLeft), key(Key::Backspace)];
        protect_raw(&ctx, &mut events, None);
        assert!(matches!(events.first(), Some(Event::Ime(ImeEvent::Preedit { text, .. })) if text == "لاَ"));
        assert!(!events.iter().any(command_key));
        assert!(events.iter().any(|e| matches!(e, Event::Key { key: Key::ArrowLeft, .. })));
        assert!(events.iter().any(|e| matches!(e, Event::Key { key: Key::Backspace, .. })));
    }

    #[test]
    fn non_page_pointer_interruption_releases_composition_keys() {
        for (multiline, drag) in [(false, false), (false, true), (true, false), (true, true)] {
            let ctx = Context::default();
            let owner = Id::unique("non-page-editor");
            let mut value = "ABC".to_owned();
            let frame = |value: &mut String, mut events: Vec<Event>, focus: bool| {
                protect_raw(&ctx, &mut events, None);
                let mut rect = egui::Rect::NOTHING;
                let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                    let editor = if multiline { egui::TextEdit::multiline(value) } else { egui::TextEdit::singleline(value) };
                    let response = editor.id(owner).desired_width(250.0).show(ui).response;
                    rect = response.rect;
                    if focus {
                        response.request_focus();
                    }
                    finish_frame(ui.ctx());
                });
                output.textures_delta.clear();
                (rect, output)
            };
            frame(&mut value, vec![], true);
            frame(&mut value, vec![], true);
            let (rect, output) = frame(&mut value, vec![preedit("مر")], false);
            assert!(composing_for(&ctx, owner));
            let committed = |output: &egui::FullOutput| {
                let egui::output::IMESurroundingText::Available(text) = &output.platform_output.ime.as_ref().unwrap().surrounding_text else {
                    panic!("short committed-text snapshot")
                };
                text.text.clone()
            };
            assert_ne!(committed(&output), value, "live preedit must reach the native editor");
            let at = rect.left_center() + egui::vec2(3.0, 0.0);
            let pointer = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            let (_, output) = if drag {
                frame(&mut value, vec![Event::PointerMoved(at), pointer(true)], false);
                frame(&mut value, vec![Event::PointerMoved(at + egui::vec2(40.0, 0.0))], false)
            } else {
                frame(&mut value, vec![Event::PointerMoved(at), pointer(true), pointer(false)], false)
            };
            assert!(ctx.interaction_snapshot(|interaction| if drag { interaction.dragged } else { interaction.clicked }) == Some(owner));
            assert_eq!(committed(&output), value, "the native editor accepted the preedit on its caret click");
            let mut events: Vec<_> = [Key::Enter, Key::Escape, Key::Tab]
                .into_iter()
                .map(|key| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE })
                .collect();
            protect_raw(&ctx, &mut events, None);
            assert_eq!(events.len(), 3, "ordinary commands must resume after a non-page editor click");
        }
    }
}
