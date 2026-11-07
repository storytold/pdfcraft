//! The keyboard as the pages see it.
//!
//! A dialog, the palette or a menu over the pages has the keys, but the pages are drawn before the
//! dialogs and read egui's keys first. So page modules read keys through [`PageKeys`], which has
//! none while something is over the pages: Delete under the Keyboard shortcuts window deleted the
//! comment selected behind it, and the Escape that closed it deselected the comment too.
//!
//! Each module still asks for itself whether a text field has the keyboard.

use egui::{Context, Event, Key, Modifiers};

/// One frame's keyboard, as the pages see it: their keys, or none while something is over them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PageKeys {
    /// A dialog, the palette or a menu is over the pages.
    over: bool,
}

impl PageKeys {
    pub(crate) fn new(over: bool) -> Self {
        Self { over }
    }

    /// The keys once something more is found over the pages this frame. Only ever more covered: a
    /// menu that this frame's Escape closes still had that Escape.
    #[must_use]
    pub(crate) fn also(self, over: bool) -> Self {
        Self { over: self.over || over }
    }

    /// `key` was pressed for the pages this frame. It stays for whatever reads it next.
    pub(crate) fn pressed(self, ctx: &Context, key: Key) -> bool {
        !self.over && ctx.input(|i| i.key_pressed(key))
    }

    /// Take `key`, pressed with no modifier, for the pages: nothing after reads it.
    pub(crate) fn take(self, ctx: &Context, key: Key) -> bool {
        !self.over && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key))
    }

    /// Take the frame's key presses that `take` wants for the pages, each judged by its own
    /// modifiers (the frame's can differ when a key is let go in the same frame). `take` says
    /// whether it took the press; a press taken is gone for whatever reads keys next.
    pub(crate) fn take_presses(self, ctx: &Context, mut take: impl FnMut(Key, Modifiers) -> bool) {
        if self.over {
            return;
        }
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                Event::Key { key, pressed: true, modifiers, .. } => !take(*key, *modifiers),
                _ => true,
            });
        });
    }

    /// Something is over the pages: their keys wait.
    pub(crate) fn over(self) -> bool {
        self.over
    }

    /// `key` is held down. Not the pages' alone: a key let go while something is over them is let
    /// go all the same.
    pub(crate) fn held(ctx: &Context, key: Key) -> bool {
        ctx.input(|i| i.key_down(key))
    }
}

#[cfg(test)]
mod tests {
    use egui::RawInput;

    use super::*;

    fn press(key: Key) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    /// Run one frame with `events`, calling `f` with its context.
    fn frame(events: Vec<Event>, f: impl FnOnce(&Context)) {
        let ctx = Context::default();
        let mut f = Some(f);
        let mut output = ctx.run_ui(RawInput { events, ..Default::default() }, |ui| {
            if let Some(f) = f.take() {
                f(ui.ctx());
            }
        });
        // An input-only test has no renderer to take the font texture.
        output.textures_delta.clear();
    }

    #[test]
    fn something_found_over_the_pages_stays_over_them_for_the_frame() {
        assert_eq!(PageKeys::new(false).also(true), PageKeys::new(true));
        assert_eq!(PageKeys::new(true).also(false), PageKeys::new(true));
        assert_eq!(PageKeys::new(false).also(false), PageKeys::new(false));
    }

    #[test]
    fn with_something_over_the_pages_they_have_no_keys_and_leave_them_there() {
        frame(vec![press(Key::Delete)], |ctx| {
            let keys = PageKeys::new(true);
            assert!(!keys.pressed(ctx, Key::Delete));
            assert!(!keys.take(ctx, Key::Delete));
            assert!(ctx.input(|i| i.key_pressed(Key::Delete)), "the key stays for the dialog");
        });
    }

    #[test]
    fn the_pages_take_the_presses_they_want_with_their_own_modifiers() {
        let shifted = Event::Key { key: Key::ArrowLeft, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::SHIFT };
        frame(vec![press(Key::ArrowRight), shifted, press(Key::Escape)], |ctx| {
            let mut seen = Vec::new();
            PageKeys::new(true).take_presses(ctx, |key, _| {
                seen.push(key);
                true
            });
            assert!(seen.is_empty(), "nothing while something is over the pages");
            PageKeys::new(false).take_presses(ctx, |key, modifiers| {
                seen.push(key);
                key == Key::ArrowLeft && modifiers.shift
            });
            assert_eq!(seen, [Key::ArrowRight, Key::ArrowLeft, Key::Escape]);
            assert!(!ctx.input(|i| i.key_pressed(Key::ArrowLeft)), "the press taken is gone");
            assert!(ctx.input(|i| i.key_pressed(Key::ArrowRight) && i.key_pressed(Key::Escape)), "the others stay");
        });
    }

    #[test]
    fn the_pages_peek_at_a_key_or_take_it() {
        frame(vec![press(Key::Delete)], |ctx| {
            let keys = PageKeys::new(false);
            assert!(keys.pressed(ctx, Key::Delete));
            assert!(keys.pressed(ctx, Key::Delete), "a peek leaves it");
            assert!(keys.take(ctx, Key::Delete));
            assert!(!keys.pressed(ctx, Key::Delete), "a take doesn't");
            assert!(!keys.take(ctx, Key::Escape));
        });
    }
}
