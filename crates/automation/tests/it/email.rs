//! `doc_email` end to end: File ▸ Attach to email… without the window. No email program is ever
//! asked: a stand-in takes the request and answers as the test says (it is a plain function, so
//! the answer is set per test thread).

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use pdfcraft_automation::{Automation, Content, ToolError};
use pdfcraft_platform::mail::{self, Answer, Compose, MailError, Why};
use serde_json::{Value, json};

thread_local! {
    /// What the stand-in was asked to attach, and what it answers next.
    static SENT: RefCell<Vec<Compose>> = const { RefCell::new(Vec::new()) };
    static ANSWER: RefCell<Option<Result<Answer, ()>>> = const { RefCell::new(None) };
    /// What the stand-in refused to take.
    static REFUSED: RefCell<Vec<Compose>> = const { RefCell::new(Vec::new()) };
}

fn stand_in(compose: Compose, _wake: Box<dyn FnOnce() + Send>) -> Result<Receiver<Answer>, MailError> {
    let (tx, rx) = channel();
    let next = ANSWER.with(|a| a.borrow_mut().take());
    let reply = match next {
        Some(Err(())) => {
            REFUSED.with(|r| r.borrow_mut().push(compose));
            return Err(MailError::CouldNotStart("refused".into()));
        }
        Some(Ok(answer)) => answer,
        None => Answer::Opened { program: Some("Stand-in Mail".into()) },
    };
    tx.send(match reply {
        Answer::NotAttached { program, why, .. } => Answer::NotAttached { program, why, fallback: mail::fallback(&compose) },
        opened => opened,
    })
    .unwrap();
    SENT.with(|s| s.borrow_mut().push(compose));
    Ok(rx)
}

fn sent() -> Vec<Compose> {
    SENT.with(|s| s.borrow().clone())
}

fn answer_with(a: Result<Answer, ()>) {
    ANSWER.with(|x| *x.borrow_mut() = Some(a));
}

fn pdf() -> Vec<u8> {
    pdfcraft_engine::Session::new().create_from_text("Invoice 26340", "Hello").unwrap().as_ref().clone()
}

/// A folder holding `name.pdf`, and an automation session confined to it with the file open as
/// document 1.
fn session(test: &str) -> (Automation, PathBuf) {
    session_with(test, true)
}

/// The same, confined to the folder or not (a copy for the email program goes to the system's
/// temp folder, which a confined session may not write to).
fn session_with(test: &str, confined: bool) -> (Automation, PathBuf) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-email-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Invoice 26340.pdf");
    std::fs::write(&path, pdf()).unwrap();
    let path = dir.canonicalize().unwrap().join("Invoice 26340.pdf");
    let mut a = Automation::new().with_mail_sender(stand_in);
    if confined {
        a = a.with_root(&dir).unwrap();
    }
    call(&mut a, "doc_open", json!({ "path": if confined { "Invoice 26340.pdf".to_string() } else { path.to_string_lossy().into_owned() } }));
    (a, path)
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

#[test]
fn an_unchanged_file_goes_as_it_is() {
    let (mut a, path) = session("unchanged");
    let r = call(&mut a, "doc_email", json!({ "doc": 1, "to": ["ap@example.org"], "body": "Attached." }));
    assert_eq!((r["opened"].clone(), r["program"].clone(), r["copy"].clone()), (json!(true), json!("Stand-in Mail"), json!(false)));
    assert_eq!(r["subject"], "Invoice 26340", "the document's name");
    let asked = sent();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].path, path, "the file itself");
    assert_eq!((asked[0].to.clone(), asked[0].body.as_str()), (vec!["ap@example.org".to_string()], "Attached."));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Unsaved edits go as a copy under the temp folder; the open file stays as it was.
#[test]
fn unsaved_edits_go_as_a_copy_and_the_open_file_is_left_alone() {
    let (mut a, path) = session_with("edited", false);
    let before = std::fs::read(&path).unwrap();
    call(&mut a, "page_rotate", json!({ "doc": 1, "pages": [1], "degrees": 90 }));
    let r = call(&mut a, "doc_email", json!({ "doc": 1, "subject": "Rotated" }));
    assert_eq!((r["opened"].clone(), r["copy"].clone(), r["subject"].clone()), (json!(true), json!(true), json!("Rotated")));
    let copy = &sent()[0].path;
    assert_ne!(copy, &path);
    assert!(copy.starts_with(std::env::temp_dir()) && copy.ends_with("Invoice 26340.pdf"), "{}", copy.display());
    let written = std::fs::read(copy).unwrap();
    assert!(written.windows(10).any(|w| w == b"/Rotate 90"), "the copy has the rotation");
    assert_eq!(std::fs::read(&path).unwrap(), before, "the open file is unchanged");
    let _ = std::fs::remove_dir_all(copy.parent().unwrap());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A session confined to a folder writes nothing outside it: an edited document is not copied to
/// the temp folder, and the caller is told to save it first, after which the saved file goes.
#[test]
fn a_confined_session_sends_only_a_saved_file() {
    let (mut a, path) = session("confined");
    call(&mut a, "page_rotate", json!({ "doc": 1, "pages": [1], "degrees": 90 }));
    let e = a.call("doc_email", &json!({ "doc": 1 })).unwrap_err();
    assert!(matches!(&e, ToolError::InvalidArgs(m) if m.contains("doc_save")), "{e}");
    assert!(sent().is_empty());
    call(&mut a, "doc_save", json!({ "doc": 1 }));
    let r = call(&mut a, "doc_email", json!({ "doc": 1 }));
    assert_eq!((r["opened"].clone(), r["copy"].clone()), (json!(true), json!(false)));
    assert_eq!(sent()[0].path, path, "the saved file itself");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A program that can't take the file: the tool still succeeds, saying why and what to offer.
#[test]
fn a_program_that_cant_take_the_file_says_why_and_what_to_offer() {
    let (mut a, path) = session("cant");
    answer_with(Ok(Answer::NotAttached {
        program: Some("Firefox".into()),
        why: Why::CantAttach,
        fallback: mail::fallback(&Compose { path: PathBuf::new(), subject: String::new(), body: String::new(), to: Vec::new() }),
    }));
    let r = call(&mut a, "doc_email", json!({ "doc": 1 }));
    assert_eq!((r["opened"].clone(), r["why"].clone(), r["program"].clone()), (json!(false), json!("cant_attach"), json!("Firefox")));
    assert_eq!(r["mailto"], "mailto:?subject=Invoice%2026340");
    assert_eq!(r["reveal"], json!(path));

    answer_with(Ok(Answer::NotAttached { program: None, why: Why::Failed("MAPI error 2".into()), fallback: mail::fallback(&sent()[0]) }));
    let r = call(&mut a, "doc_email", json!({ "doc": 1 }));
    assert_eq!((r["why"].clone(), r["detail"].clone()), (json!("failed"), json!("MAPI error 2")));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Redaction marks that aren't applied hide nothing in the file: refused unless the caller says it
/// knows, and then sent.
#[test]
fn unapplied_redaction_marks_are_refused_unless_allowed() {
    let (mut a, path) = session_with("marks", false);
    call(&mut a, "redact_mark", json!({ "doc": 1, "page": 1, "rect": [10, 10, 100, 40] }));
    let e = a.call("doc_email", &json!({ "doc": 1 })).unwrap_err();
    assert!(matches!(&e, ToolError::InvalidArgs(m) if m.contains("redaction marks")), "{e}");
    assert!(sent().is_empty(), "nothing was asked");
    let r = call(&mut a, "doc_email", json!({ "doc": 1, "allow_unapplied_redactions": true }));
    assert_eq!(r["opened"], true);
    assert_eq!(sent().len(), 1);
    let _ = std::fs::remove_dir_all(sent()[0].path.parent().unwrap());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A request refused before anything was asked leaves no copy behind; a bad address is refused as
/// one.
#[test]
fn a_refused_request_leaves_no_copy_and_bad_arguments_are_refused() {
    let (mut a, path) = session_with("refused", false);
    call(&mut a, "page_rotate", json!({ "doc": 1, "pages": [1], "degrees": 90 }));
    answer_with(Err(()));
    let e = a.call("doc_email", &json!({ "doc": 1 })).unwrap_err();
    assert!(matches!(e, ToolError::Failed(_)), "{e}");
    assert!(sent().is_empty());
    let refused = REFUSED.with(|r| r.borrow().clone());
    assert_eq!(refused.len(), 1, "the request was made");
    assert!(!refused[0].path.exists() && !refused[0].path.parent().unwrap().exists(), "its copy and the folder are gone");

    for bad in [json!({ "doc": 1, "to": "ap@example.org" }), json!({ "doc": 1, "to": [1] }), json!({ "doc": 1, "subject": 5 }), json!({})] {
        assert!(matches!(a.call("doc_email", &bad), Err(ToolError::InvalidArgs(_))), "{bad}");
    }
    assert!(matches!(a.call("doc_email", &json!({ "doc": 9 })), Err(ToolError::Failed(_))), "no such document");
    // The stand-in never sees a recipient the mail module would refuse; the module's own check
    // runs in the real sender, so here the refusal is the stand-in's.
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// The tool is the command's headless form: `command_run` reaches it, and `command_list` names it.
#[test]
fn the_command_runs_through_command_run() {
    let (mut a, path) = session("command");
    let r = call(&mut a, "command_run", json!({ "id": "file.send_email", "params": { "doc": 1, "to": ["ap@example.org"] } }));
    assert_eq!(r["opened"], true);
    assert_eq!(sent().len(), 1);
    let listed = call(&mut a, "command_list", json!({ "filter": "file.send_email" }));
    let row = listed["commands"].as_array().unwrap().iter().find(|c| c["id"] == "file.send_email").expect("listed");
    assert_eq!(row["tool"], "doc_email");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
