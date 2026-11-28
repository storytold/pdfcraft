use super::PdfCraftApp;

use craft::ui::runtime::frame::FrameInput;
use craft::ui::shell::embed;

impl PdfCraftApp {
    /// The app's part of [`eframe::App::raw_input_hook`], before egui reads the frame's input:
    /// the runtime hands over what it gathered since the last frame (the commands fired) and takes this frame's root values,
    /// command states and host. eframe calls the hook once per frame, before the first pass, so
    /// a second pass of the same frame runs on what this call set and fires nothing twice.
    pub(super) fn prepare_raw_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if raw.viewport_id == egui::ViewportId::ROOT {
            let (pointer, down) = ctx.input_for(raw.viewport_id, |i| (i.pointer.latest_pos(), i.pointer.any_down()));
            // A hidden window's frames run no pass to land in, and a press beside one that is held hovers nothing: the
            // held press has the pointer.
            let waits = !down && raw.viewport().visible() != Some(false);
            let lands = |at: egui::Pos2| waits && Some(at) != pointer && self.views.iter().any(|view| view.viewport_rect().contains(at));
            if hold_landing_press(&mut raw.events, &mut self.landing, lands) {
                ctx.request_repaint_of(raw.viewport_id);
            }
        }
        self.set_title_bar_choice(ctx, self.title_bar_choice);
        self.control_before_input(ctx);
        embed::before_input(ctx, |output| {
            for fired in &output.fired {
                self.run_fired(ctx, fired);
            }
            for declined in &output.declined {
                self.run_declined(declined);
            }
            self.sync_host_locale();
            let commands = crate::commands::catalog::command_input(self, ctx);
            FrameInput::new(self.root_env(ctx)).commands(commands).host(self.host.clone())
        });
        self.last_frame_active = self.active_ids().map(|(_, id)| id);
    }

    fn sync_host_locale(&mut self) {
        let Some(catalogs) = self.host.i18n() else { return };
        let language = crate::i18n::Lang::from_pref(&self.language);
        let date_language = self.session.date_language().unwrap_or(language.code());
        if self.host_date_language.as_deref() != Some(date_language) {
            catalogs.set_dates(craft_i18n::DateFormat::new(&craft_i18n::Region::from_tag(date_language)));
            self.host_date_language = Some(date_language.to_string());
        }
        let lang = catalogs.from_code(language.code()).unwrap_or(craft_i18n::Lang::EN);
        if self.host.lang() != lang {
            self.host = std::sync::Arc::new(self.host.with_lang(lang));
        }
    }
}

/// Lets the pointer land on the pages a frame before it presses. A press resolves over what the last frame showed, and
/// a comment is a part of its page only under the pointer, so a press that comes with the pointer's arrival, as every
/// tap's does and a fast mouse's can, would go past the comment to what lies under it. From the first press that
/// `lands` on, `events` wait in `held` and lead the next frame's, in order; the frame gets the move alone. A press
/// waits one frame at most, so another press held behind it, at another place, comes with it. Returns whether events
/// wait.
fn hold_landing_press(events: &mut Vec<egui::Event>, held: &mut Vec<egui::Event>, lands: impl Fn(egui::Pos2) -> bool) -> bool {
    // What waited a frame comes now, every press in it: only the frame's own events can wait.
    let landed = held.len();
    held.append(events);
    std::mem::swap(events, held);
    let landing = events.iter().enumerate().skip(landed).find_map(|(at, event)| match event {
        egui::Event::PointerButton { pos, pressed: true, .. } if lands(*pos) => Some((at, *pos)),
        _ => None,
    });
    let Some((at, pos)) = landing else { return false };
    *held = events.split_off(at);
    // Not every backend moves the pointer to a press first.
    let moved = events.iter().rev().find_map(|event| match event {
        egui::Event::PointerMoved(to) => Some(*to),
        _ => None,
    });
    if moved != Some(pos) {
        events.push(egui::Event::PointerMoved(pos));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::hold_landing_press;
    use egui::{Event, Key, Modifiers, PointerButton, pos2};

    /// A press that lands gets a frame of its own move first, also where the backend sent none (a browser's touch),
    /// then comes with every event behind it in order, and waits no longer than that, nor does a press at another
    /// place that came behind it.
    #[test]
    fn a_landing_press_waits_one_frame_behind_its_move_and_keeps_the_order() {
        let (at, beside) = (pos2(5.0, 6.0), pos2(40.0, 6.0));
        let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        let key = Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        let sent = vec![
            button(at, true),
            button(at, false),
            Event::PointerMoved(beside),
            button(beside, true),
            button(beside, false),
            key.clone(),
            Event::PointerGone,
        ];
        let mut held = Vec::new();
        let mut first = sent.clone();
        assert!(hold_landing_press(&mut first, &mut held, |_| true));
        assert_eq!(first, [Event::PointerMoved(at)]);
        let mut next = vec![Event::WindowFocused(false)];
        assert!(!hold_landing_press(&mut next, &mut held, |_| true), "neither press waits a second frame");
        assert_eq!(next, [sent, vec![Event::WindowFocused(false)]].concat());
        assert!(held.is_empty());
    }
}
