use craft::behavior::IconId;
use craft::theme::variant::Tone;
use craft::ui::dialog::{DialogId, DialogResult, PromptAnswer, PromptChoice, StandardPrompt};
use pdfcraft_engine::DocId;

use super::PdfCraftApp;

/// The host dialogs of the document prompts that are open, with what each one needs to be answered. A prompt is
/// opened from its own pending state and taken by its id, so several can be open at once.
#[derive(Default)]
pub(crate) struct OpenPrompts {
    /// The Full Screen question and the document it is about.
    pub(crate) full_screen: Option<(DialogId, DocId)>,
    /// The Revert question.
    pub(crate) revert: Option<DialogId>,
    /// The question about opening `pending_link`.
    pub(crate) link: Option<DialogId>,
    /// The question about opening `pending_remote_link`.
    pub(crate) remote_link: Option<DialogId>,
    /// The Apply redactions question.
    pub(crate) apply_redactions: Option<DialogId>,
    /// The Sanitize question.
    pub(crate) sanitize: Option<DialogId>,
    /// The question about locking placed signatures into the page before a save (`pending_lock`).
    pub(crate) lock_signatures: Option<DialogId>,
    /// The confirmation question before deleting pages.
    pub(crate) delete_pages: Option<DialogId>,
}

/// `name` cut to at most `max` characters by replacing its middle with "…", keeping the start and
/// the end, where the extension and version suffixes sit. Counts `char`s, so it never splits one.
pub(crate) fn shorten_middle(name: &str, max: usize) -> String {
    let count = name.chars().count();
    if count <= max {
        return name.to_string();
    }
    let tail = max / 4;
    let head: String = name.chars().take(max.saturating_sub(tail + 1)).collect();
    let end: String = name.chars().skip(count.saturating_sub(tail)).collect();
    format!("{head}…{end}")
}

/// Whether a taken answer chose Proceed: `None` while the prompt has not answered, `Some(false)` for any other
/// button, Escape or a close.
fn proceeded(answer: Option<DialogResult<PromptAnswer>>) -> Option<bool> {
    answer.map(|result| matches!(result, DialogResult::Ok(answer) if answer.choice == PromptChoice::Proceed))
}

impl PdfCraftApp {
    /// A document that asks to open in Full Screen mode (`/PageMode /FullScreen`) is asked about
    /// first, as Acrobat does: full screen hides every control, so it is the person's choice.
    pub(crate) fn ask_full_screen(&mut self, id: DocId) {
        let prompt = StandardPrompt::custom(tl!(
            "This document asks to open in Full Screen mode: the page fills the screen and nothing else shows. Press Esc at any time to leave it."
        ))
        .title(tl!("Open in Full Screen mode?"))
        .icon(IconId::new("maximize"))
        .button(PromptChoice::Proceed, tl!("Full Screen"))
        .button(PromptChoice::Cancel, tl!("Not now"))
        .default_choice(PromptChoice::Cancel)
        .tag(id)
        .instance("full-screen");
        let dialog = self.dialogs.open(prompt);
        self.prompts.full_screen = Some((dialog, id));
    }

    /// "Revert" over the active document: Proceed runs `revert_active`.
    pub(crate) fn ask_revert(&mut self) {
        let name = self.active_ids().and_then(|(_, id)| self.session.get(id)).map(|d| d.name.clone()).unwrap_or_default();
        let message = crate::i18n::fmt(
            tl!("Revert to the previously saved version of “{name}”? Changes since then can't be undone afterwards."),
            &[("name", &name)],
        );
        let prompt = StandardPrompt::custom(message)
            .title(tl!("Revert"))
            .button(PromptChoice::Proceed, tl!("Revert"))
            .button(PromptChoice::Cancel, tl!("Cancel"))
            .instance("revert");
        self.prompts.revert = Some(self.dialogs.open(prompt));
    }

    /// "Open this web page?" when a document's link, button or script asks to open an address (#90, #91). Shows
    /// where the address really goes and the whole address; Cancel is the default.
    pub(crate) fn ask_link(&mut self) {
        let Some(pending) = self.pending_link.clone() else { return };
        let email = pending.url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("mailto:"));
        let (title, open) = if email { (tl!("Write this email?"), tl!("Open email app")) } else { (tl!("Open this web page?"), tl!("Open link")) };
        let template = if email {
            tl!("{who} in this document wants to open your email app with this address:")
        } else {
            tl!("{who} in this document wants to open this address in your browser:")
        };
        let mut prompt = StandardPrompt::custom(crate::i18n::fmt(template, &[("who", tl!(pending.origin.noun()))]))
            .title(title)
            .icon(IconId::new("external-link"))
            .button(PromptChoice::Proceed, open)
            .button(PromptChoice::Cancel, tl!("Cancel"))
            .default_choice(PromptChoice::Cancel)
            .instance("link");
        // The host as the browser will connect to it, in punycode when it is international, so a
        // lookalike such as `pаypal.com` (Cyrillic `а`) reads as `xn--pypal-4ve.com`. Its Unicode
        // form isn't repeated here: a whole-script lookalike would read as the real site.
        if let Some(host) = pdfcraft_engine::links::display_host(&pending.url) {
            prompt = prompt.paragraph(host.ascii.clone());
            if host.mixed_scripts {
                prompt = prompt
                    .tone(Tone::Bad, tl!("This web address mixes letters from different alphabets, a common way to imitate another site's name."));
            } else if host.international {
                prompt = prompt.tone(Tone::Warn, tl!("This web address uses letters from another alphabet, which can look like familiar ones."));
            }
        }
        let prompt = prompt.lines([pending.url.clone().into()]).note(tl!(
            "Only continue if you trust this document. An address can carry information from the document, such as what you typed into its form."
        ));
        self.prompts.link = Some(self.dialogs.open(prompt));
    }

    /// Absolute document-controlled local paths require consent before any filesystem access.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn ask_remote_link(&mut self) {
        let Some(pending) = self.pending_remote_link.as_ref() else { return };
        // A local path reads left to right in every language, so its final component stays last.
        let path = craft_i18n::ltr(&pending.path.to_string_lossy());
        let prompt = StandardPrompt::custom(tl!("This document is trying to open a local file:"))
            .title(tl!("Open this local PDF?"))
            .paragraph(path)
            .paragraph(tl!("Only allow this if you trust the source document."))
            .button(PromptChoice::Proceed, tl!("Allow"))
            .button(PromptChoice::Cancel, tl!("Cancel"))
            .default_choice(PromptChoice::Cancel)
            .instance("remote-link");
        self.prompts.remote_link = Some(self.dialogs.open(prompt));
    }

    /// Ask before deleting pages.
    pub(crate) fn ask_delete_pages(&mut self, pages: &[usize]) {
        let n = pages.len();
        let message = if n == 1 {
            tl!("Are you Sure you want to delete this page?").to_string()
        } else {
            crate::i18n::fmt(tl!("Are you Sure you want to delete these {n} pages?"), &[("n", &n.to_string())])
        };
        let prompt = StandardPrompt::custom(message)
            .title(tl!("Delete page?"))
            .button(PromptChoice::Proceed, tl!("Delete"))
            .button(PromptChoice::Cancel, tl!("Cancel"))
            .default_choice(PromptChoice::Proceed)
            .instance("delete-pages");
        self.prompts.delete_pages = Some(self.dialogs.open(prompt));
    }

    /// One pass over the document prompts: takes each open prompt's answer, cancels a prompt whose question is
    /// gone, then opens the prompts that are pending. An answer is taken once, so a second pass in a frame
    /// applies nothing twice.
    pub(crate) fn prompts_pass(&mut self, ctx: &egui::Context) {
        if let Some((dialog, doc)) = self.prompts.full_screen {
            if let Some(allow) = proceeded(self.dialogs.take_by_id::<StandardPrompt>(dialog)) {
                self.prompts.full_screen = None;
                self.resolve_full_screen_prompt(doc, allow);
            } else if self.session.get(doc).is_none() {
                // The document was closed before an answer: nothing to ask any more.
                self.dialogs.cancel_by_id(ctx, dialog);
                self.prompts.full_screen = None;
            }
        }
        if let Some(dialog) = self.prompts.revert
            && let Some(go) = proceeded(self.dialogs.take_by_id::<StandardPrompt>(dialog))
        {
            self.prompts.revert = None;
            if go {
                self.revert_active(ctx);
            }
        }
        if let Some(dialog) = self.prompts.link {
            if let Some(open) = proceeded(self.dialogs.take_by_id::<StandardPrompt>(dialog)) {
                self.prompts.link = None;
                self.resolve_pending_link(open);
            } else if self.pending_link.is_none() {
                self.dialogs.cancel_by_id(ctx, dialog);
                self.prompts.link = None;
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dialog) = self.prompts.remote_link {
            if let Some(open) = proceeded(self.dialogs.take_by_id::<StandardPrompt>(dialog)) {
                self.prompts.remote_link = None;
                self.resolve_pending_remote_link(open);
            } else if self.pending_remote_link.is_none() {
                self.dialogs.cancel_by_id(ctx, dialog);
                self.prompts.remote_link = None;
            }
        }
        if let Some(dialog) = self.prompts.apply_redactions
            && let Some(go) = proceeded(self.dialogs.take_by_id::<StandardPrompt>(dialog))
        {
            self.prompts.apply_redactions = None;
            if go {
                crate::redact_ui::finish_apply(ctx, self);
            }
        }
        if let Some(dialog) = self.prompts.sanitize
            && let Some(go) = proceeded(self.dialogs.take_by_id::<StandardPrompt>(dialog))
        {
            self.prompts.sanitize = None;
            if go {
                crate::redact_ui::finish_sanitize(ctx, self);
            }
        }
        if let Some(dialog) = self.prompts.lock_signatures
            && let Some(result) = self.dialogs.take_by_id::<StandardPrompt>(dialog)
        {
            self.prompts.lock_signatures = None;
            match result {
                DialogResult::Ok(answer) => super::signature::finish_lock(ctx, self, answer),
                _ => self.pending_lock = None,
            }
        }
        if let Some(dialog) = self.prompts.delete_pages {
            if let Some(go) = proceeded(self.dialogs.take_by_id::<StandardPrompt>(dialog)) {
                self.prompts.delete_pages = None;
                if go {
                    self.confirm_delete_pages(ctx);
                } else {
                    self.delete_pages_draft = None;
                }
            } else if self.delete_pages_draft.as_ref().is_none_or(|d| self.session.get(d.doc).is_none()) {
                self.dialogs.cancel_by_id(ctx, dialog);
                self.prompts.delete_pages = None;
                self.delete_pages_draft = None;
            }
        }
        if self.pending_link.is_some() && self.prompts.link.is_none() {
            self.ask_link();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.pending_remote_link.is_some() && self.prompts.remote_link.is_none() {
            self.ask_remote_link();
        }
    }
}
