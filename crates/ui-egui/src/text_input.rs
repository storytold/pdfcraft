//! Keep composition confirmation/cancellation keys out of document commands.
//!
//! Text stays logical and egui owns shaping, grapheme motion and the preedit range.
//! This remembers only which focused widget has an unfinished composition.

use egui::{Context, Event, Id, ImeEvent, Key};

#[derive(Clone, Copy, Default)]
struct Composition {
    owner: Option<Id>,
    active: bool,
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
                Event::WindowFocused(false) => self.active = false,
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
pub(crate) fn protect_raw(ctx: &Context, events: &mut Vec<Event>) {
    let owner = focused_editor(ctx);
    let mut composition = ctx.data(|d| d.get_temp::<Composition>(state_id())).unwrap_or_default();
    if composition.observe(owner, events) {
        events.retain(|event| !command_key(event));
    }
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
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::TextEdit::singleline(&mut value).id(owner).show(ui).response.request_focus();
            });
        }
        let key = |key| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let mut events = vec![preedit("لاَ"), key(Key::Enter), key(Key::Escape), key(Key::Tab), key(Key::ArrowLeft), key(Key::Backspace)];
        protect_raw(&ctx, &mut events);
        assert!(matches!(events.first(), Some(Event::Ime(ImeEvent::Preedit { text, .. })) if text == "لاَ"));
        assert!(!events.iter().any(command_key));
        assert!(events.iter().any(|e| matches!(e, Event::Key { key: Key::ArrowLeft, .. })));
        assert!(events.iter().any(|e| matches!(e, Event::Key { key: Key::Backspace, .. })));
    }
}
