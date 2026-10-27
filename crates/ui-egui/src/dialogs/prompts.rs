use super::*;

/// The buttons of the "Save changes?" prompt, in the order they sit on screen, left to right.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PromptButton {
    DontSave,
    Cancel,
    #[default]
    Save,
}

impl PromptButton {
    const ORDER: [PromptButton; 3] = [PromptButton::DontSave, PromptButton::Cancel, PromptButton::Save];

    fn position(self) -> usize {
        Self::ORDER.iter().position(|b| *b == self).unwrap_or(2)
    }

    /// The button `by` places to the right (negative: left). Arrow keys stop at the ends.
    fn step(self, by: isize) -> PromptButton {
        let to = self.position().saturating_add_signed(by).min(Self::ORDER.len() - 1);
        Self::ORDER.get(to).copied().unwrap_or(self)
    }

    /// The button after (or, with `back`, before) this one, wrapping round: Tab and Shift+Tab.
    fn cycle(self, back: bool) -> PromptButton {
        let n = Self::ORDER.len();
        let to = if back { self.position().checked_sub(1).unwrap_or(n - 1) } else { (self.position() + 1) % n };
        Self::ORDER.get(to).copied().unwrap_or(self)
    }

    /// What pressing the button answers.
    fn answer(self) -> Option<bool> {
        match self {
            PromptButton::Save => Some(true),
            PromptButton::DontSave => Some(false),
            PromptButton::Cancel => None,
        }
    }
}

/// The "Save changes?" prompt's keys (issue #8), read before anything else can take them. Save is
/// the button chosen when the prompt opens, so Enter saves; the arrow keys and Tab / Shift+Tab move
/// the choice, and Enter or Space press the chosen button. Escape cancels. Don't save also has
/// ⌘D / Ctrl+D, the macOS convention, and Alt+D / Alt+N, the mnemonics Windows and Linux desktops
/// use for it. Returns the answer once one is given.
pub(crate) fn save_prompt_key(ctx: &egui::Context, chosen: &mut PromptButton) -> Option<Option<bool>> {
    use egui::{Key, Modifiers};
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Space) {
            return Some(chosen.answer());
        }
        if i.consume_key(Modifiers::NONE, Key::Escape) {
            return Some(None);
        }
        if [(Modifiers::COMMAND, Key::D), (Modifiers::ALT, Key::D), (Modifiers::ALT, Key::N)].into_iter().any(|(m, k)| i.consume_key(m, k)) {
            return Some(Some(false));
        }
        // Shift+Tab before Tab: a key press matches a pattern that asks for fewer modifiers.
        if i.consume_key(Modifiers::SHIFT, Key::Tab) {
            *chosen = chosen.cycle(true);
        } else if i.consume_key(Modifiers::NONE, Key::Tab) {
            *chosen = chosen.cycle(false);
        }
        for (key, by) in [(Key::ArrowLeft, -1), (Key::ArrowUp, -1), (Key::ArrowRight, 1), (Key::ArrowDown, 1)] {
            if i.consume_key(Modifiers::NONE, key) {
                *chosen = chosen.step(by);
            }
        }
        None
    })
}

/// "Save changes?" when closing a tab or quitting with unsaved edits.
pub(super) fn save_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(req) = app.close_request else { return };
    let index = match req {
        CloseRequest::Tab(id) => app.views.iter().position(|v| v.id == id),
        CloseRequest::Quit | CloseRequest::All => app.first_dirty(),
    };
    let Some(name) = index.and_then(|i| app.views.get(i)).and_then(|v| app.session.get(v.id)).map(|d| d.name.clone()) else {
        // Nothing left to ask about (tab already gone or no dirty documents).
        app.resolve_close(ctx, Some(false));
        return;
    };
    let t = Tokens::get(ctx);
    // Wrapping alone (#161) still let a long enough name (a web `?file=` URL has no limit) grow
    // the prompt taller than the window (#236); 80 characters wrap to a few lines.
    let shown = shorten_middle(&name, 80);
    let mut choice: Option<Option<bool>> = None;
    let modal = widgets::show_modal(ctx, "save_prompt", |ui| {
        ui.set_width(420.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("save", 22.0, t.accent));
            let title = ui.add(
                egui::Label::new(
                    egui::RichText::new(crate::i18n::fmt(tl!("Save changes to “{name}” before closing?"), &[("name", &shown)]))
                        .font(theme::semibold(16.0)),
                )
                .wrap(),
            );
            if shown != name {
                title.on_hover_text(&name);
            }
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(tl!("Your changes will be lost if you don't save them.")).color(t.text_muted));
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let chosen = app.save_prompt_button;
            if widgets::pill_button_marked(ui, tl!("Save"), true, chosen == PromptButton::Save).clicked() {
                choice = Some(Some(true));
            }
            if widgets::pill_button_marked(ui, tl!("Cancel"), false, chosen == PromptButton::Cancel).clicked() {
                choice = Some(None);
            }
            ui.add_space(24.0);
            if widgets::pill_button_marked(ui, tl!("Don't save"), false, chosen == PromptButton::DontSave).clicked() {
                choice = Some(Some(false));
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(None);
    }
    if let Some(c) = choice {
        app.resolve_close(ctx, c);
    }
}

/// `name` cut to at most `max` characters by replacing its middle with "…", keeping the start and
/// the end, where the extension and version suffixes sit. Counts `char`s, so it never splits one.
fn shorten_middle(name: &str, max: usize) -> String {
    let count = name.chars().count();
    if count <= max {
        return name.to_string();
    }
    let tail = max / 4;
    let head: String = name.chars().take(max.saturating_sub(tail + 1)).collect();
    let end: String = name.chars().skip(count.saturating_sub(tail)).collect();
    format!("{head}…{end}")
}

/// "Open this web page?" when a document's link, button or script asks to open an address
/// (#90, #91). Shows where the address really goes and the whole address; Cancel is the default,
/// and Escape or clicking outside cancels.
pub(super) fn link_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(pending) = app.pending_link.clone() else { return };
    let t = Tokens::get(ctx);
    let email = pending.url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("mailto:"));
    let (title, open) = if email { (tl!("Write this email?"), tl!("Open email app")) } else { (tl!("Open this web page?"), tl!("Open link")) };
    let mut choice: Option<bool> = None;
    let modal = widgets::show_modal(ctx, "link_prompt", |ui| {
        ui.set_width(460.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("external-link", 22.0, t.accent));
            ui.label(egui::RichText::new(title).font(theme::semibold(16.0)));
        });
        ui.add_space(6.0);
        let template = if email {
            tl!("{who} in this document wants to open your email app with this address:")
        } else {
            tl!("{who} in this document wants to open this address in your browser:")
        };
        ui.label(crate::i18n::fmt(template, &[("who", tl!(pending.origin.noun()))]));
        ui.add_space(6.0);
        // The host as the browser will connect to it, in punycode when it is international, so a
        // lookalike such as `pаypal.com` (Cyrillic `а`) reads as `xn--pypal-4ve.com`. Its Unicode
        // form isn't repeated here: a whole-script lookalike would read as the real site.
        if let Some(host) = pdfcraft_engine::links::display_host(&pending.url) {
            ui.label(egui::RichText::new(&host.ascii).font(theme::semibold(14.0)));
            if host.mixed_scripts {
                let warning = tl!("This web address mixes letters from different alphabets, a common way to imitate another site's name.");
                ui.add(egui::Label::new(egui::RichText::new(warning).color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B))).wrap());
            } else if host.international {
                let note = tl!("This web address uses letters from another alphabet, which can look like familiar ones.");
                ui.add(egui::Label::new(egui::RichText::new(note).color(t.text)).wrap());
            }
        }
        egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(&pending.url).monospace().small()).wrap().selectable(true));
        });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(tl!(
                "Only continue if you trust this document. An address can carry information from the document, such as what you typed into its form."
            ))
            .color(t.text_muted)
            .small(),
        );
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Cancel"), true).clicked() {
                choice = Some(false);
            }
            if widgets::pill_button(ui, open, false).clicked() {
                choice = Some(true);
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(false);
    }
    if let Some(open) = choice {
        app.resolve_pending_link(open);
    }
}

/// A document that asks to open in Full Screen mode (`/PageMode /FullScreen`) is asked about
/// first, as Acrobat does: full screen hides every control, so it is the person's choice.
pub(super) fn full_screen_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(id) = app.pending_full_screen else { return };
    // The document was closed before an answer: nothing to ask any more.
    if app.session.get(id).is_none() {
        app.pending_full_screen = None;
        return;
    }
    let t = Tokens::get(ctx);
    let mut choice = None;
    let modal = widgets::show_modal(ctx, "full_screen_prompt", |ui| {
        ui.set_width(460.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("maximize", 22.0, t.accent));
            ui.label(egui::RichText::new(tl!("Open in Full Screen mode?")).font(theme::semibold(16.0)));
        });
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(tl!(
                "This document asks to open in Full Screen mode: the page fills the screen and nothing else shows. Press Esc at any time to leave it."
            ))
            .wrap(),
        );
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Not now"), true).clicked() {
                choice = Some(false);
            }
            if widgets::pill_button(ui, tl!("Full Screen"), false).clicked() {
                choice = Some(true);
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(false);
    }
    if let Some(allow) = choice {
        app.resolve_full_screen_prompt(ctx, allow);
    }
}

/// Absolute document-controlled local paths require consent before any filesystem access.
pub(super) fn remote_link_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(pending) = app.pending_remote_link.clone() else { return };
    let mut choice = None;
    let modal = widgets::show_modal(ctx, "remote_link_prompt", |ui| {
        ui.set_width(460.0);
        ui.label(egui::RichText::new(tl!("Open this local PDF?")).font(theme::semibold(16.0)));
        ui.add_space(6.0);
        ui.label(tl!("This document is trying to open a local file:"));
        egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
            // A local path is file/path chrome: left to right, so its final component stays last.
            let mut job = egui::text::LayoutJob::simple(
                pending.path.to_string_lossy().into_owned(),
                egui::FontId::monospace(13.0),
                crate::theme::Tokens::get(ui.ctx()).text,
                ui.available_width().max(0.0),
            );
            job.direction = egui::epaint::text::TextDirection::LeftToRight;
            let galley = ui.painter().layout_job(job);
            ui.add(egui::Label::new(egui::WidgetText::Galley(galley)).selectable(true));
        });
        ui.add_space(8.0);
        ui.label(tl!("Only allow this if you trust the source document."));
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Cancel"), true).clicked() {
                choice = Some(false);
            }
            if widgets::pill_button(ui, tl!("Allow"), false).clicked() {
                choice = Some(true);
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(false);
    }
    if let Some(open) = choice {
        app.resolve_pending_remote_link(open);
    }
}

/// What a password box was answered with.
enum PasswordAnswer {
    Submit,
    Cancel,
}

/// A password box: a lock, a title, a message, the masked field, an error and two buttons.
fn password_modal(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    message: String,
    input: &mut String,
    error: Option<&str>,
    ok: &str,
) -> Option<PasswordAnswer> {
    let t = Tokens::get(ctx);
    let mut answer = None;
    let modal = widgets::show_modal(ctx, id, |ui| {
        ui.set_width(400.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("lock", 22.0, t.accent));
            ui.label(egui::RichText::new(title).font(theme::semibold(17.0)));
        });
        ui.add_space(6.0);
        ui.add(egui::Label::new(message).wrap());
        ui.add_space(8.0);
        let r = ui.add(widgets::line(input).password(true).hint_text(tl!("Password")).desired_width(f32::INFINITY));
        // Enter submits. The field keeps focus, so check the key while it is focused as well as
        // on the frame focus is lost.
        if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            answer = Some(PasswordAnswer::Submit);
        }
        // Only when it lost focus: each request interrupts the input method, and on Linux typed
        // keys go through it, so asking on every frame lost everything typed (#2818).
        if !r.has_focus() {
            r.request_focus();
        }
        if let Some(e) = error {
            ui.add_space(4.0);
            // Known error text is translated; anything else passes through.
            ui.label(egui::RichText::new(tl!(e)).color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B)));
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, ok, true).clicked() {
                answer = Some(PasswordAnswer::Submit);
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                answer = Some(PasswordAnswer::Cancel);
            }
        });
    });
    answer.or(modal.should_close().then_some(PasswordAnswer::Cancel))
}

/// Password prompt for encrypted documents (Acrobat: "Password" dialog on open).
pub(super) fn password(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(prompt) = app.password_prompt.as_mut() else { return };
    let message = crate::i18n::fmt(tl!("“{name}” is protected. Enter a password to open it."), &[("name", &prompt.name)]);
    match password_modal(ctx, "password", tl!("Password required"), message, &mut prompt.input, prompt.error.as_deref(), tl!("Open")) {
        Some(PasswordAnswer::Submit) => {
            let pw = prompt.input.clone();
            app.submit_password(Some(pw));
        }
        Some(PasswordAnswer::Cancel) => app.submit_password(None),
        None => {}
    }
}

/// Permissions password prompt: lifts the restrictions of an open document (#995).
pub(super) fn unlock(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(prompt) = app.unlock_prompt.as_mut() else { return };
    let message =
        crate::i18n::fmt(tl!("“{name}” has security restrictions. Enter its permissions password to lift them."), &[("name", &prompt.name)]);
    match password_modal(ctx, "unlock", tl!("Permissions password"), message, &mut prompt.input, prompt.error.as_deref(), tl!("Unlock")) {
        Some(PasswordAnswer::Submit) => {
            let pw = prompt.input.clone();
            app.submit_unlock(Some(pw));
        }
        Some(PasswordAnswer::Cancel) => app.submit_unlock(None),
        None => {}
    }
}

#[cfg(test)]
mod prompt_button_tests {
    use super::PromptButton::{self, Cancel, DontSave, Save};

    #[test]
    fn opens_on_save_and_arrows_stop_at_the_ends() {
        assert_eq!(PromptButton::default(), Save);
        assert_eq!(Save.step(1), Save);
        assert_eq!(Save.step(-1), Cancel);
        assert_eq!(Cancel.step(-1), DontSave);
        assert_eq!(DontSave.step(-1), DontSave);
        assert_eq!(DontSave.step(1000), Save, "a long step clamps");
        assert_eq!(Save.step(isize::MIN), DontSave, "and so does a huge negative one");
    }

    #[test]
    fn tab_wraps_both_ways() {
        assert_eq!(Save.cycle(false), DontSave);
        assert_eq!(DontSave.cycle(true), Save);
        assert_eq!(Cancel.cycle(false), Save);
        assert_eq!(Cancel.cycle(true), DontSave);
    }

    #[test]
    fn each_button_answers_as_labelled() {
        assert_eq!(Save.answer(), Some(true));
        assert_eq!(DontSave.answer(), Some(false));
        assert_eq!(Cancel.answer(), None);
    }
}
