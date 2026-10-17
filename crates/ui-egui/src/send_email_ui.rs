//! File ▸ Send by email…: the document as it is now, attached to a new message in the computer's
//! default email app (`pdfcraft_platform::mail`), the way Acrobat's *Attach to Email* works.
//! Nothing is sent from here and nothing goes over the network: the message opens in the email app
//! for the person to address and send.
//!
//! An unchanged file goes as it is; one with unsaved edits goes as a copy with those edits,
//! written under the temp folder, so nobody is asked to save first and the open file stays as it
//! was. While the email app is asked, a notice says so (and a second request only says it is
//! still waiting). When the file can't be attached (a webmail site or another app that can't be
//! handed a file, no email app, an app that didn't answer), a dialog says why and offers a new
//! message without the file, with the file shown in its folder to attach by hand.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Align, Layout, RichText};
use pdfcraft_platform::mail::{self, Answer, Compose, MailError, Why};

use crate::theme::{self, Tokens};
use crate::{Dialog, PdfCraftApp, widgets};

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
    crate::tab_context::reveal(path)
}

#[cfg(target_arch = "wasm32")]
fn reveal(_path: &Path) -> Result<(), String> {
    Err("File reveal is not available on this platform".into())
}

/// Send by email's state on the app (`app.send_email`).
pub struct SendEmailState {
    pub sender: Sender,
    pub reveal: Reveal,
    waiting: Option<Waiting>,
    /// The file that couldn't be attached, for the dialog.
    pub problem: Option<Problem>,
}

impl Default for SendEmailState {
    fn default() -> Self {
        Self { sender: send, reveal, waiting: None, problem: None }
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
    notice: String,
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
    pub(crate) fn send_by_email(&mut self) {
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
        if !self.commit_typing_in(id) {
            return;
        }
        let Some(doc) = self.session.get(id) else { return };
        let (name, path, dirty) = (doc.name.clone(), doc.path.clone(), doc.dirty);
        let attach = match path.filter(|p| !dirty && Path::new(p).is_file()) {
            Some(p) => PathBuf::from(p),
            None => {
                let bytes = match self.session.save_bytes(id) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        self.notify_error(e);
                        return;
                    }
                };
                let dir = copy_dir();
                let target = dir.join(attachment_name(&name));
                if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&target, &*bytes)) {
                    self.notify_fmt("The copy to attach couldn't be written: {e}", &[("e", &e.to_string())]);
                    return;
                }
                target
            }
        };
        let compose = Compose { path: attach, subject: subject_for(&name), body: String::new(), to: Vec::new() };
        let ctx = self.ctx.clone();
        let wake: Box<dyn FnOnce() + Send> = Box::new(move || {
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        match (self.send_email.sender)(compose.clone(), wake) {
            Ok(rx) => {
                let notice = crate::i18n::fmt(tl!("Attaching {name} to a new email message…"), &[("name", &name)]);
                self.notify(notice.clone());
                self.send_email.waiting = Some(Waiting { rx, notice, compose });
            }
            Err(MailError::Unsupported) => self.notify_tr("Sending by email isn't available in the browser"),
            Err(e) => self.notify_error(e),
        }
    }

    /// Each frame: the email app's answer, if it has come. Opened: the notice fades by itself as
    /// the message comes up. Not attached: the dialog.
    pub(crate) fn poll_send_email(&mut self) {
        let Some(w) = &self.send_email.waiting else { return };
        let answer = match w.rx.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => {
                // Keep the notice up while it is the one showing (a notice fades after a while).
                if let Some((shown, age)) = self.toast.as_mut()
                    && *shown == w.notice
                {
                    *age = 0.0;
                }
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
            // The waiting notices give way to the dialog.
            if self.toast.as_ref().is_some_and(|(shown, _)| *shown == w.notice || shown == tl!("Still waiting for your email app…")) {
                self.toast = None;
            }
            self.send_email.problem = Some(Problem { program, why, compose: w.compose, fallback });
            self.dialog = Some(Dialog::SendEmail);
        }
    }

    /// The dialog's main button: a new message without the file, and the file in its folder.
    fn open_message_and_show_file(&mut self, problem: &Problem) {
        // The address goes through the check a document's addresses get; one it refuses (too
        // long, or a name that reads like an instruction to some email app) goes without the
        // subject and body.
        let url = pdfcraft_engine::links::document_url(&problem.fallback.mailto).unwrap_or_else(|_| mail::mailto(&problem.compose.to, "", ""));
        self.open_url(&url);
        self.show_in_folder(&problem.fallback.reveal);
    }

    fn show_in_folder(&mut self, path: &Path) {
        if let Err(e) = (self.send_email.reveal)(path) {
            self.notify_error(e);
        }
    }
}

/// The "couldn't attach" dialog (`Dialog::SendEmail`). Returns whether it closes.
pub(crate) fn problem_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    let Some(problem) = app.send_email.problem.clone() else { return true };
    let name = file_name(&problem.fallback.reveal);
    ui.label(RichText::new(tl!("Couldn't attach the file")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(wording(&problem, &name));
    ui.add_space(10.0);
    egui::Frame::NONE
        .stroke(egui::Stroke::new(1.0, t.divider))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(&name).color(t.text));
        });
    ui.add_space(12.0);
    let mut close = false;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("Open message and show file"), true).clicked() {
            app.open_message_and_show_file(&problem);
            close = true;
        }
        let reveal = if cfg!(target_os = "macos") { tl!("Reveal in Finder") } else { tl!("Reveal in File Explorer") };
        if widgets::pill_button(ui, reveal, false).clicked() {
            app.show_in_folder(&problem.fallback.reveal);
            close = true;
        }
        if widgets::pill_button(ui, tl!("Close"), false).clicked() {
            close = true;
        }
    });
    if close {
        app.send_email.problem = None;
    }
    close
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
        (Why::Failed(why), _) => fmt(tl!("Your email app didn't open a message. It said: {why}."), &[("why", why.trim().trim_end_matches('.'))]),
    };
    let offer =
        fmt(tl!("Open a new message without it, and PdfCraft will show {name} in its folder so you can attach it yourself."), &[("name", name)]);
    format!("{what} {offer}")
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Where copies with unsaved edits are written: a folder of PdfCraft's under the system's temp
/// folder, one file per document name (a later send of the same document replaces it).
pub fn copy_dir() -> PathBuf {
    std::env::temp_dir().join("pdfcraft-send-email")
}

/// The attachment's file name: the document's, ending in .pdf, with nothing a path would read.
fn attachment_name(name: &str) -> String {
    let base: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '\0') || c.is_control() { '_' } else { c }).collect();
    let base = base.trim().trim_start_matches('.');
    let base = if base.trim().is_empty() { "Document" } else { base };
    if base.to_ascii_lowercase().ends_with(".pdf") { base.to_string() } else { format!("{base}.pdf") }
}

/// The message's subject: the document's name without its extension.
fn subject_for(name: &str) -> String {
    let n = name.trim();
    let stem =
        n.len().checked_sub(4).filter(|&i| n.get(i..).is_some_and(|ext| ext.eq_ignore_ascii_case(".pdf"))).and_then(|i| n.get(..i)).unwrap_or(n);
    let stem = stem.trim();
    if stem.is_empty() { "Document".to_string() } else { stem.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_subjects() {
        assert_eq!(attachment_name("Invoice 26340.pdf"), "Invoice 26340.pdf");
        assert_eq!(attachment_name("Invoice 26340"), "Invoice 26340.pdf");
        assert_eq!(attachment_name("a/b:c.PDF"), "a_b_c.PDF");
        assert_eq!(attachment_name("   "), "Document.pdf");
        assert_eq!(attachment_name("..pdf"), "pdf.pdf");
        assert_eq!(subject_for("Invoice 26340.pdf"), "Invoice 26340");
        assert_eq!(subject_for("letter.PDF"), "letter");
        assert_eq!(subject_for(".pdf"), "Document");
        assert_eq!(subject_for("Año"), "Año");
        assert_eq!(subject_for("ñ.pdf"), "ñ");
    }

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
            (Why::NoProgram, None, "No email app is set up on this computer."),
            (Why::NoAnswer, Some("Mail"), "Your email app didn't open a message in time."),
            (Why::LogonFailed, None, "Your email app couldn't log on to your mailbox."),
            (Why::Failed("Access denied.".into()), None, "Your email app didn't open a message. It said: Access denied."),
        ] {
            let text = wording(&problem(why, program), "Factura 26340.pdf");
            assert_eq!(text, format!("{start} {offer}"));
        }
    }
}
