//! `doc_email`: File ▸ Attach to email… without the window. The document as it is now goes into a
//! new message in the computer's default email program (`pdfcraft_platform::mail`), which the
//! person then addresses and sends; nothing is sent from here and nothing goes over the network.

use std::sync::mpsc::Receiver;
use std::time::Duration;

use pdfcraft_platform::attachment::{remove_copy, subject_for, write_private_copy};
use pdfcraft_platform::mail::{self, Answer, Compose, MailError, Why};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed};

/// What starts a request: [`mail::send`]. A test puts a stand-in here, so no email program is
/// asked (the `--root` and `with_mail_sender` of an embedding do the same).
pub type MailSender = fn(Compose, Box<dyn FnOnce() + Send>) -> std::result::Result<Receiver<Answer>, MailError>;

pub(crate) fn real_sender(compose: Compose, wake: Box<dyn FnOnce() + Send>) -> std::result::Result<Receiver<Answer>, MailError> {
    mail::send(compose, wake)
}

/// A little over the longest wait the mail module itself allows an email program.
const ANSWER_LIMIT: Duration = Duration::from_secs(mail::ANSWER_SECONDS + 30);

/// `why` as the tool names it, and the program's own words when it has them.
fn why_code(why: &Why) -> (&'static str, Option<&str>) {
    match why {
        Why::CantAttach => ("cant_attach", None),
        Why::NoProgram => ("no_program", None),
        Why::NoAnswer => ("no_answer", None),
        Why::LogonFailed => ("logon_failed", None),
        Why::Failed(detail) => ("failed", Some(detail)),
    }
}

impl Automation {
    /// `doc_email`: attach the document to a new email message.
    pub(crate) fn doc_email(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let (id, name, path, dirty) = (doc.id, doc.name.clone(), doc.path.clone(), doc.dirty);
        // Redaction marks that aren't applied hide nothing in the file, so what is under them
        // would go with it: the caller says it knows.
        if doc.redaction_marks() > 0 && !a.opt_bool("allow_unapplied_redactions")?.unwrap_or(false) {
            return Err(ToolError::InvalidArgs(
                "the document has redaction marks that aren't applied, so the content under them is still in the file; apply them (redact_apply) or pass allow_unapplied_redactions: true"
                    .into(),
            ));
        }
        let to: Vec<String> = match a.get("to") {
            None | Some(Value::Null) => Vec::new(),
            Some(_) => a.strs("to")?.into_iter().map(str::to_string).collect(),
        };
        let subject = a.opt_str("subject")?.map_or_else(|| subject_for(&name), str::to_string);
        let body = a.opt_str("body")?.unwrap_or_default().to_string();

        // An unchanged file goes as it is; one with edits goes as a copy that has them, so the
        // open file is left alone.
        let mut copy = None;
        let attach = match path.filter(|p| !dirty && std::path::Path::new(p).is_file()) {
            Some(p) => p.into(),
            None => {
                // The copy goes to the system's temp folder, outside any folder this session is
                // confined to, so a confined session sends only a file it has saved.
                if self.root.is_some() || self.roots.as_ref().is_some_and(|r| r.write_root().is_some()) {
                    return Err(ToolError::InvalidArgs(
                        "the document has unsaved edits, and this session only writes inside its write root, so a copy for the email program can't be made; save the document first (doc_save)"
                            .into(),
                    ));
                }
                let bytes = self.session.save_bytes(id)?;
                let target = write_private_copy(&name, &bytes).map_err(|e| failed(format!("the copy to attach couldn't be written: {e}")))?;
                copy = Some(target.clone());
                target
            }
        };
        let compose = Compose { path: attach.clone(), subject: subject.clone(), body, to: to.clone() };
        let rx = match (self.mail_sender)(compose.clone(), Box::new(|| {})) {
            Ok(rx) => rx,
            Err(e) => {
                // Refused before anything was asked: the copy was never handed over.
                remove_copy(copy.as_deref());
                return Err(match e {
                    MailError::Unsupported => failed("sending by email isn't available on this system"),
                    MailError::NoFile(_) | MailError::BadRecipient(_) => ToolError::InvalidArgs(e.to_string()),
                    e => failed(e),
                });
            }
        };
        let answer = rx.recv_timeout(ANSWER_LIMIT).map_err(|_| failed("the email program gave no answer"))?;
        let sent = json!({ "attached": attach, "copy": copy.is_some(), "subject": subject, "to": to });
        let mut out = sent.as_object().cloned().unwrap_or_default();
        match answer {
            Answer::Opened { program } => {
                out.insert("opened".into(), json!(true));
                out.insert("program".into(), json!(program));
            }
            Answer::NotAttached { program, why, fallback } => {
                // The file stays where it is, for the caller to attach by hand.
                let (code, detail) = why_code(&why);
                out.insert("opened".into(), json!(false));
                out.insert("program".into(), json!(program));
                out.insert("why".into(), json!(code));
                if let Some(detail) = detail {
                    out.insert("detail".into(), json!(detail));
                }
                out.insert("mailto".into(), json!(fallback.mailto));
                out.insert("reveal".into(), json!(fallback.reveal));
            }
        }
        Ok(Value::Object(out))
    }
}
