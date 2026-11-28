//! File ▸ Attach to email…: the document as it is now, attached to a new message in the computer's
//! default email app (`pdfcraft_platform::mail`), the way Acrobat's *Attach to Email* works.
//! Nothing is sent from here and nothing goes over the network: the message opens in the email app
//! for the person to address and send.
//!
//! An unchanged file goes as it is; one with unsaved edits goes as a copy with those edits,
//! written to a new folder of its own under the temp folder, so nobody is asked to save first and
//! the open file stays as it was. While the email app is asked, a notice says so (and a second
//! request only says it is still waiting). When the file can't be attached (a webmail site or
//! another app that can't be handed a file, no email app, an app that didn't answer), a dialog
//! says why and offers a new message without the file, with the file shown in its folder to attach
//! by hand. A document with redaction marks that aren't applied asks first: the marks hide nothing
//! in the file, so what is under them would go with it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use craft::ui::dialog::{DialogId, DialogResult, PromptAnswer, PromptChoice, StandardPrompt};
use craft::ui::notices::PostedNotice;
use craft::ui::runtime::toast::ToastId;
use pdfcraft_platform::attachment::{file_holds, remove_copy, subject_for, write_private_copy};
use pdfcraft_platform::mail::{self, Answer, Compose, MailError, Why};

use crate::PdfCraftApp;
use pdfcraft_engine::DocId;

/// What starts a request: [`mail::send`]. A test puts a stand-in here, so nothing on the machine
/// running the tests is asked.
pub type Sender = fn(Compose, Box<dyn FnOnce() + Send>) -> Result<Receiver<Answer>, MailError>;

fn send(compose: Compose, wake: Box<dyn FnOnce() + Send>) -> Result<Receiver<Answer>, MailError> {
    mail::send(compose, wake)
}

/// What shows a file in its folder (the file manager). A test puts a stand-in here too.
pub type Reveal = fn(&Path) -> Result<(), String>;

#[cfg(not(target_arch = "wasm32"))]
fn reveal(path: &Path) -> Result<(), String> {
    craft::platform::open::reveal(path)
}

#[cfg(target_arch = "wasm32")]
fn reveal(_path: &Path) -> Result<(), String> {
    Err("File reveal is not available on this platform".into())
}

/// What opens a `mailto:` address in the email app: [`mail::open_address`]. A test puts a
/// stand-in here as well.
pub type AddressOpener = fn(&str, Box<dyn FnOnce() + Send>) -> Result<Receiver<Option<Why>>, MailError>;

fn open_address(url: &str, wake: Box<dyn FnOnce() + Send>) -> Result<Receiver<Option<Why>>, MailError> {
    mail::open_address(url, wake)
}

/// Attach to email's state on the app (`app.send_email`).
pub struct SendEmailState {
    pub sender: Sender,
    pub reveal: Reveal,
    pub open_address: AddressOpener,
    waiting: Option<Waiting>,
    /// A `mailto:` address the email app is being asked to open, until it answers.
    opening: Option<(String, Receiver<Option<Why>>)>,
    /// The file that couldn't be attached, for the dialog.
    pub problem: Option<Problem>,
    /// The "couldn't attach" dialog on the host, while it is open.
    problem_prompt: Option<DialogId>,
    /// The document the person chose to send with its redaction marks unapplied.
    send_with_marks: Option<DocId>,
    /// The unapplied redactions question on the host and the document it was asked about.
    marks_prompt: Option<(DialogId, DocId)>,
}

impl Default for SendEmailState {
    fn default() -> Self {
        Self {
            sender: send,
            reveal,
            open_address,
            waiting: None,
            opening: None,
            problem: None,
            problem_prompt: None,
            send_with_marks: None,
            marks_prompt: None,
        }
    }
}

impl SendEmailState {
    /// A request is under way.
    pub fn waiting(&self) -> bool {
        self.waiting.is_some()
    }
}

/// A request the email app hasn't answered yet, and the notice that shows meanwhile.
struct Waiting {
    rx: Receiver<Answer>,
    /// The notice that says the email app is being asked, kept up until it answers.
    card: Option<ToastId>,
    compose: Compose,
}

/// Why the file wasn't attached, and the message without it to offer instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub program: Option<String>,
    pub why: Why,
    pub compose: Compose,
    pub fallback: mail::Fallback,
}

impl PdfCraftApp {
    /// Attach the active document to a new message in the default email app.
    pub(crate) fn send_by_email(&mut self, ctx: &egui::Context) {
        if cfg!(target_arch = "wasm32") {
            self.notify_tr("Sending by email isn't available in the browser");
            return;
        }
        let Some((_, id)) = self.active_ids() else {
            self.notify_tr("Open a document first");
            return;
        };
        if self.send_email.waiting.is_some() {
            self.notify_tr("Still waiting for your email app…");
            return;
        }
        // What's typed in a field is part of what goes, as for Save.
        if !self.commit_typing_in(ctx, id) {
            return;
        }
        let Some(doc) = self.session.get(id) else { return };
        if doc.redaction_marks() > 0 && self.send_email.send_with_marks.take() != Some(id) {
            self.ask_send_with_marks(id);
            return;
        }
        let (name, path, dirty) = (doc.name.clone(), doc.path.clone(), doc.dirty);
        let mut copy = None;
        // What the person sees: the open document as it is now (its own bytes when it has no
        // unsaved edits). The file on disk goes only when it is exactly that; a clean document
        // whose file was replaced since it was opened (automatic reload off) goes as a copy of
        // the open version, never as the replacement.
        let bytes = match self.session.save_bytes(id) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.notify_error(e);
                return;
            }
        };
        let attach = match path.filter(|p| !dirty && file_holds(Path::new(p), &bytes)) {
            Some(p) => PathBuf::from(p),
            None => match write_private_copy(&name, &bytes) {
                Ok(target) => {
                    copy = Some(target.clone());
                    target
                }
                Err(e) => {
                    self.notify_fmt("The copy to attach couldn't be written: {e}", &[("e", &e.to_string())]);
                    return;
                }
            },
        };
        let compose = Compose { path: attach, subject: subject_for(&name), body: String::new(), to: Vec::new() };
        let wake = self.wake.clone();
        let wake: Box<dyn FnOnce() + Send> = Box::new(move || wake.wake());
        match (self.send_email.sender)(compose.clone(), wake) {
            Ok(rx) => {
                let notice = crate::i18n::fmt(tl!("Attaching {name} to a new email message…"), &[("name", &name)]);
                let card = self.notices.post(ctx, &PostedNotice::new(notice), &[]);
                self.send_email.waiting = Some(Waiting { rx, card, compose });
            }
            // Refused before anything was asked: the copy was never handed over, so it goes.
            Err(MailError::Unsupported) => {
                remove_copy(copy.as_deref());
                self.notify_tr("Sending by email isn't available in the browser");
            }
            Err(e) => {
                remove_copy(copy.as_deref());
                self.notify_error(e);
            }
        }
    }

    /// Each frame: the email app's answer, if it has come. Opened: the notice fades by itself as
    /// the message comes up. Not attached: the dialog.
    pub(crate) fn poll_send_email(&mut self, ctx: &egui::Context) {
        self.poll_open_address();
        let Some(w) = &self.send_email.waiting else { return };
        // Keep the notice up while the email app is asked (a notice fades after a while).
        let keep_notice = |app: &mut Self| {
            if let Some(card) = app.send_email.waiting.as_ref().and_then(|w| w.card) {
                app.notices.keep(ctx, card);
            }
        };
        // A dialog or prompt (a save question, a password) is open: leave the answer in the
        // channel until it closes, so nothing is replaced or covered without going through its
        // own close path.
        if self.modal_open() {
            keep_notice(self);
            return;
        }
        let answer = match w.rx.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => {
                keep_notice(self);
                return;
            }
            Err(TryRecvError::Disconnected) => Answer::NotAttached {
                program: None,
                why: Why::Failed(tl!("the request stopped without an answer").to_string()),
                fallback: mail::fallback(&w.compose),
            },
        };
        let Some(w) = self.send_email.waiting.take() else { return };
        if let Answer::NotAttached { program, why, fallback } = answer {
            // The waiting notice gives way to the dialog.
            if let Some(card) = w.card {
                self.notices.remove(ctx, card);
            }
            let problem = Problem { program, why, compose: w.compose, fallback };
            self.ask_about_problem(&problem);
            self.send_email.problem = Some(problem);
        }
    }

    /// Asks before sending a document whose redaction marks aren't applied: what is under them
    /// would go with it.
    fn ask_send_with_marks(&mut self, id: DocId) {
        let prompt = StandardPrompt::custom(tl!(
            "This document has redaction marks that aren't applied. The content under them is still in the file, and anyone who receives it can read it."
        ))
        .title(tl!("Redactions aren't applied"))
        .button(PromptChoice::Proceed, tl!("Send anyway"))
        .button(PromptChoice::Cancel, tl!("Cancel"))
        .default_choice(PromptChoice::Cancel)
        .instance("send-email-marks");
        self.send_email.marks_prompt = Some((self.dialogs.open(prompt), id));
    }

    /// The "couldn't attach" dialog: what happened, the file, and what can be done instead. Where
    /// no email app is set up a message can't be opened, so only the file is offered.
    fn ask_about_problem(&mut self, problem: &Problem) {
        let name = file_name(&problem.fallback.reveal);
        let reveal = if cfg!(target_os = "macos") { tl!("Reveal in Finder") } else { tl!("Reveal in File Explorer") };
        let mut prompt = StandardPrompt::custom(wording(problem, &name)).title(tl!("Couldn't attach the file")).lines([name.into()]);
        prompt = if problem.why == Why::NoProgram {
            prompt.button(PromptChoice::Proceed, reveal)
        } else {
            prompt.button(PromptChoice::Proceed, tl!("Open message and show file")).button(PromptChoice::Alternate, reveal)
        };
        let prompt = prompt.button(PromptChoice::Cancel, tl!("Close")).instance("send-email-problem");
        self.send_email.problem_prompt = Some(self.dialogs.open(prompt));
    }

    /// The answers of the two dialogs, once given.
    pub(crate) fn send_email_pass(&mut self, ctx: &egui::Context) {
        if let Some((dialog, asked)) = self.send_email.marks_prompt
            && let Some(answer) = self.dialogs.take_by_id::<StandardPrompt>(dialog)
        {
            self.send_email.marks_prompt = None;
            // The consent is for the document the question was asked about, not whichever is
            // active by now.
            if choice(answer) == Some(PromptChoice::Proceed) && self.active_ids().map(|(_, id)| id) == Some(asked) {
                self.send_email.send_with_marks = Some(asked);
                self.send_by_email(ctx);
                self.send_email.send_with_marks = None;
            }
        }
        if let Some(dialog) = self.send_email.problem_prompt
            && let Some(answer) = self.dialogs.take_by_id::<StandardPrompt>(dialog)
        {
            self.send_email.problem_prompt = None;
            let Some(problem) = self.send_email.problem.take() else { return };
            let only_file = problem.why == Why::NoProgram;
            match choice(answer) {
                Some(PromptChoice::Proceed) if only_file => self.show_in_folder(&problem.fallback.reveal),
                Some(PromptChoice::Proceed) => self.open_message_and_show_file(&problem),
                Some(PromptChoice::Alternate) => self.show_in_folder(&problem.fallback.reveal),
                _ => {}
            }
        }
    }

    /// The dialog's main button: a new message without the file, and the file in its folder.
    fn open_message_and_show_file(&mut self, problem: &Problem) {
        // The address goes through the check a document's addresses get; one it refuses (too
        // long, or a name that reads like an instruction to some email app) goes without the
        // subject and body.
        let url = pdfcraft_engine::links::document_url(&problem.fallback.mailto).unwrap_or_else(|_| mail::mailto(&problem.compose.to, "", ""));
        self.open_email_address(&url);
        self.show_in_folder(&problem.fallback.reveal);
    }

    /// Open the `mailto:` address `url` in the email app the person set (a document's email link
    /// once allowed, the message without the file): through the system's handler for `mailto:`
    /// links ([`mail::open_address`]), not as a web link, which macOS and Windows would hand to the
    /// web browser. Where that can't be asked, or it fails, the address opens as a web link after
    /// all, so a click is never lost.
    pub(crate) fn open_email_address(&mut self, url: &str) {
        let wake = self.wake.clone();
        let wake = Box::new(move || wake.wake());
        match (self.send_email.open_address)(url, wake) {
            Ok(rx) => {
                self.send_email.opening = Some((url.to_string(), rx));
            }
            Err(_) => self.open_url(url),
        }
    }

    /// The email app's answer to [`Self::open_email_address`]: nothing to do when it opened;
    /// otherwise the address opens as a web link.
    fn poll_open_address(&mut self) {
        let Some((url, rx)) = &self.send_email.opening else { return };
        let failed = match rx.try_recv() {
            Ok(None) => false,
            Ok(Some(_)) | Err(TryRecvError::Disconnected) => true,
            Err(TryRecvError::Empty) => return,
        };
        let url = url.clone();
        self.send_email.opening = None;
        if failed {
            self.open_url(&url);
        }
    }

    fn show_in_folder(&mut self, path: &Path) {
        if let Err(e) = (self.send_email.reveal)(path) {
            self.notify_error(e);
        }
    }
}

/// The button a dialog's answer chose; `None` for Escape or a close.
fn choice(answer: DialogResult<PromptAnswer>) -> Option<PromptChoice> {
    match answer {
        DialogResult::Ok(answer) => Some(answer.choice),
        _ => None,
    }
}

/// What happened, then what can be done instead.
fn wording(problem: &Problem, name: &str) -> String {
    let fmt = crate::i18n::fmt;
    let what = match (&problem.why, &problem.program) {
        (Why::CantAttach, Some(program)) => fmt(tl!("{program} can't take a file from PdfCraft."), &[("program", program)]),
        (Why::CantAttach, None) => tl!("Your default email app can't take a file from PdfCraft.").to_string(),
        (Why::NoProgram, _) => tl!("No email app is set up on this computer.").to_string(),
        (Why::NoAnswer, _) => tl!("Your email app didn't open a message in time.").to_string(),
        (Why::LogonFailed, _) => tl!("Your email app couldn't log on to your mailbox.").to_string(),
        (Why::Busy, _) => tl!("Your email app is still showing the message from your last request. Send or close it, then try again.").to_string(),
        (Why::Failed(why), _) => fmt(tl!("Your email app didn't open a message. It said: {why}."), &[("why", why.trim().trim_end_matches('.'))]),
    };
    let offer = if problem.why == Why::NoProgram {
        fmt(tl!("PdfCraft can show {name} in its folder so you can attach it yourself."), &[("name", name)])
    } else {
        fmt(tl!("Open a new message without it, and PdfCraft will show {name} in its folder so you can attach it yourself."), &[("name", name)])
    };
    format!("{what} {offer}")
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(why: Why, program: Option<&str>) -> Problem {
        let compose = Compose { path: PathBuf::from("/x/Factura 26340.pdf"), subject: "Factura 26340".into(), body: String::new(), to: Vec::new() };
        Problem { program: program.map(str::to_string), why, fallback: mail::fallback(&compose), compose }
    }

    #[test]
    fn each_problem_says_what_happened_and_what_can_be_done() {
        crate::i18n::set_current(crate::i18n::Lang::from_pref("en"));
        let offer = "Open a new message without it, and PdfCraft will show Factura 26340.pdf in its folder so you can attach it yourself.";
        for (why, program, start) in [
            (Why::CantAttach, Some("Firefox"), "Firefox can't take a file from PdfCraft."),
            (Why::CantAttach, None, "Your default email app can't take a file from PdfCraft."),
            (Why::NoAnswer, Some("Mail"), "Your email app didn't open a message in time."),
            (Why::LogonFailed, None, "Your email app couldn't log on to your mailbox."),
            (
                Why::Busy,
                Some("Microsoft Outlook"),
                "Your email app is still showing the message from your last request. Send or close it, then try again.",
            ),
            (Why::Failed("Access denied.".into()), None, "Your email app didn't open a message. It said: Access denied."),
        ] {
            let text = wording(&problem(why, program), "Factura 26340.pdf");
            assert_eq!(text, format!("{start} {offer}"));
        }
        // With no email app a message can't be opened, so only the file is offered.
        assert_eq!(
            wording(&problem(Why::NoProgram, None), "Factura 26340.pdf"),
            "No email app is set up on this computer. PdfCraft can show Factura 26340.pdf in its folder so you can attach it yourself."
        );
    }
}
