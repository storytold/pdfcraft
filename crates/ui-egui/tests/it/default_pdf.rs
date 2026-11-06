//! Synthetic host callbacks: no OS associations are read, written, or launched by these tests.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_engine::{Edit, Protection, Session};
use pdfcraft_ui_egui::{DefaultPdfIntegration, PdfCraftApp};

const QUESTION: &str = "Make PdfCraft your default PDF app?";

fn pdf() -> Vec<u8> {
    Session::new().create_blank(200.0, 300.0, 1).unwrap().as_ref().clone()
}

fn host(answer: Result<bool, &'static str>, automatic_prompt: bool) -> (DefaultPdfIntegration, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let launches = Arc::new(AtomicUsize::new(0));
    let read_count = reads.clone();
    let launch_count = launches.clone();
    (
        DefaultPdfIntegration {
            is_default: Arc::new(move || {
                read_count.fetch_add(1, Ordering::SeqCst);
                answer.map_err(str::to_string)
            }),
            open_settings: Arc::new(move || {
                launch_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
            automatic_prompt,
        },
        reads,
        launches,
    )
}

fn harness(integration: Option<DefaultPdfIntegration>) -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1100.0, 800.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.default_pdf_integration = integration;
        app
    })
}

fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    // Give the query worker a bounded chance to complete, including error/already-default cases.
    for _ in 0..30 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn default_pdf_only_successful_pdf_opens_trigger_once_per_session() {
    let (source, reads, launches) = host(Ok(false), true);
    let mut h = harness(Some(source));
    h.run_steps(3);
    assert_eq!(reads.load(Ordering::SeqCst), 0, "empty launch");
    assert!(h.state_mut().execute("create.blank"));
    h.state_mut().open_bytes("notes.txt", None, b"A newly created PDF".to_vec()).unwrap();
    assert!(h.state_mut().open_bytes("broken.pdf", None, b"%PDF-1.7\nbroken".to_vec()).is_err());
    settle(&mut h);
    assert_eq!(reads.load(Ordering::SeqCst), 0, "Create/conversion/failed open do not ask");
    h.state_mut().open_bytes("existing.pdf", None, pdf()).unwrap();
    settle(&mut h);
    h.get_by_label(QUESTION);
    if let Some(output) = std::env::var_os("PDFCRAFT_DEFAULT_PDF_SCREENSHOT") {
        h.state_mut().set_option("language", "de").unwrap();
        h.run_steps(3);
        h.get_by_label("PdfCraft als Standardprogramm für PDFs verwenden?");
        h.render().unwrap().save(&output).unwrap();
        h.state_mut().set_option("theme", "dark").unwrap();
        h.run_steps(3);
        h.render().unwrap().save(std::path::PathBuf::from(&output).with_extension("dark.png")).unwrap();
        h.state_mut().set_option("theme", "light").unwrap();
        h.state_mut().set_option("language", "en").unwrap();
        h.run_steps(3);
    }
    h.get_by_label("Later").click();
    h.run_steps(3);
    h.state_mut().open_bytes("second.pdf", None, pdf()).unwrap();
    settle(&mut h);
    assert!(h.query_by_label(QUESTION).is_none());
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(launches.load(Ordering::SeqCst), 0);
}

#[test]
fn default_pdf_password_must_succeed_and_other_dialogs_finish_first() {
    let (source, reads, _) = host(Ok(false), true);
    let mut h = harness(Some(source));
    let mut session = Session::new();
    let id = session.open("locked.pdf", None, Arc::new(pdf()), None).unwrap();
    session.apply(id, Edit::Protect(Protection { open_password: Some("test-password".into()), ..Default::default() })).unwrap();
    h.state_mut().open_bytes("locked.pdf", None, session.save_bytes(id).unwrap().as_ref().clone()).unwrap();
    settle(&mut h);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    h.state_mut().submit_password(Some("wrong".into()));
    settle(&mut h);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    h.state_mut().submit_password(Some("test-password".into()));
    h.state_mut().set_option("dialog", "preferences").unwrap();
    settle(&mut h);
    assert!(h.query_by_label(QUESTION).is_none());
    h.state_mut().dialog = None;
    h.run_steps(3);
    h.get_by_label(QUESTION);
}

#[test]
fn default_pdf_checkbox_persists_for_yes_later_close_and_escape() {
    for answer in ["Yes", "Later", "Close", "Escape"] {
        let (source, _, launches) = host(Ok(false), true);
        let mut h = harness(Some(source.clone()));
        h.state_mut().open_bytes("existing.pdf", None, pdf()).unwrap();
        settle(&mut h);
        h.get_by_label("Do not ask again").click();
        h.run_steps(2);
        if answer == "Escape" {
            h.key_press(egui::Key::Escape);
        } else {
            h.get_by_label(answer).click();
        }
        h.run_steps(3);
        assert!(h.query_by_label(QUESTION).is_none());
        assert_eq!(launches.load(Ordering::SeqCst), usize::from(answer == "Yes"));
        let saved = h.state().persist();
        let mut restarted = harness(Some(source));
        restarted.state_mut().restore(&saved);
        assert!(!restarted.state().ask_default_pdf);
        restarted.state_mut().open_bytes("later.pdf", None, pdf()).unwrap();
        settle(&mut restarted);
        assert!(restarted.query_by_label(QUESTION).is_none());
        restarted.state_mut().set_option("dialog", "preferences").unwrap();
        restarted.run_steps(3);
        restarted.get_by_label("Ask to make PdfCraft the default PDF app").scroll_to_me();
        restarted.run_steps(20);
        let option = restarted.get_by_label("Ask to make PdfCraft the default PDF app").rect();
        let card = restarted.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("dialog").with("card"))).unwrap();
        assert!(restarted.ctx.content_rect().contains_rect(option), "preference is inside the viewport");
        assert!(card.contains_rect(option), "preference is inside the dialog");
        restarted.run_steps(1);
        assert_eq!(restarted.get_by_label("Ask to make PdfCraft the default PDF app").rect(), option, "preference stays in place before clicking");
        restarted.get_by_label("Ask to make PdfCraft the default PDF app").click();
        restarted.run_steps(2);
        assert!(restarted.state().ask_default_pdf, "Preferences re-enables the persistent preference");
    }
}

#[test]
fn default_pdf_already_default_failed_query_disabled_and_non_windows_hosts_do_not_ask() {
    for answer in [Ok(true), Err("registry unavailable")] {
        let (source, _, launches) = host(answer, true);
        let mut h = harness(Some(source));
        h.state_mut().open_bytes("existing.pdf", None, pdf()).unwrap();
        settle(&mut h);
        assert!(h.query_by_label(QUESTION).is_none());
        assert_eq!(launches.load(Ordering::SeqCst), 0);
    }
    for integration in [None, Some(host(Ok(false), false).0)] {
        let mut h = harness(integration);
        h.state_mut().open_bytes("existing.pdf", None, pdf()).unwrap();
        settle(&mut h);
        assert!(h.query_by_label(QUESTION).is_none(), "automation and non-Windows hosts are quiet");
    }
    let (source, reads, _) = host(Ok(false), true);
    let mut h = harness(Some(source));
    h.state_mut().set_option("ask-default-pdf", "off").unwrap();
    h.state_mut().open_bytes("existing.pdf", None, pdf()).unwrap();
    settle(&mut h);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(h.state_mut().set_option("ask-default-pdf", "invalid").is_err());
}

#[test]
fn default_pdf_settings_failures_preserve_checkbox_and_document() {
    let (mut source, _, _) = host(Ok(false), true);
    source.open_settings = Arc::new(|| Err("synthetic launch failure".into()));
    let mut h = harness(Some(source));
    h.state_mut().open_bytes("existing.pdf", None, pdf()).unwrap();
    settle(&mut h);
    h.get_by_label("Do not ask again").click();
    h.run_steps(2);
    h.get_by_label("Yes").click();
    h.run_steps(3);
    assert!(!h.state().ask_default_pdf);
    assert_eq!(h.state().views.len(), 1);
    assert!(h.state().toast.as_ref().unwrap().0.contains("synthetic launch failure"));
}

#[test]
fn default_pdf_control_command_and_preference_are_explicit_and_testable() {
    let (source, reads, launches) = host(Ok(false), false);
    let mut h = harness(Some(source));
    let client = h.state_mut().attach_control(&egui::Context::default());
    let request = client.send("ui.set", serde_json::json!({"key":"ask-default-pdf", "value":"off"}));
    h.run_steps(3);
    assert!(request.try_recv().unwrap().is_ok());
    assert!(!h.state().ask_default_pdf);
    h.state_mut().open_bytes("automated.pdf", None, pdf()).unwrap();
    let request = client.send("ui.command", serde_json::json!({"id":"app.default_pdf_settings"}));
    h.run_steps(3);
    assert!(request.try_recv().unwrap().is_ok());
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    let request = client.send("ui.state", serde_json::json!({}));
    h.run_steps(3);
    let state = request.try_recv().unwrap().unwrap();
    assert_eq!(state["ask_default_pdf"], false);
    assert_eq!(state["default_pdf_prompt"], false);
}

#[test]
fn default_pdf_ipc_open_uses_the_same_once_per_session_gate() {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!("pdfcraft-default-pdf-ipc-{}.pdf", std::process::id()));
    let mut file = std::fs::File::create_new(&path).unwrap();
    file.write_all(&pdf()).unwrap();
    drop(file);
    let (source, reads, _) = host(Ok(false), true);
    let mut h = harness(Some(source));
    let mut events = Some(vec![pdfcraft_ui_egui::OsEvent::Open(vec![path.to_string_lossy().into_owned()])]);
    h.state_mut().os_events = Some(Box::new(move || events.take().unwrap_or_default()));
    settle(&mut h);
    h.get_by_label(QUESTION);
    h.get_by_label("Later").click();
    h.run_steps(3);
    h.state_mut().open_path(&path.to_string_lossy());
    settle(&mut h);
    assert!(h.query_by_label(QUESTION).is_none());
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn default_pdf_offer_waits_for_explorer_email_review_to_finish() {
    let (source, reads, launches) = host(Ok(false), true);
    let mut h = harness(Some(source));
    h.state_mut().open_bytes("existing.pdf", None, pdf()).unwrap();
    h.state_mut().receive_shell(pdfcraft_ui_egui::ShellAction::Email, &["synthetic-attachment.pdf".into()]);
    settle(&mut h);
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert!(h.query_by_label(QUESTION).is_none(), "do not stack the default-app offer over the email review");
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    h.get_by_label(QUESTION);
    assert_eq!(launches.load(Ordering::SeqCst), 0, "neither OS settings nor a mail client is opened");
}

#[test]
fn explorer_email_review_waits_for_an_already_open_default_offer() {
    let (source, _, launches) = host(Ok(false), true);
    let mut h = harness(Some(source));
    h.state_mut().open_bytes("existing.pdf", None, pdf()).unwrap();
    settle(&mut h);
    h.get_by_label(QUESTION);
    h.state_mut().receive_shell(pdfcraft_ui_egui::ShellAction::Email, &["synthetic-attachment.pdf".into()]);
    h.run_steps(3);
    assert!(h.query_by_label("Save email draft…").is_none());
    h.get_by_label("Later").click();
    h.run_steps(3);
    h.get_by_label("Save email draft…");
    assert_eq!(h.state().shell_email_paths.len(), 1);
    assert_eq!(launches.load(Ordering::SeqCst), 0);
}

#[test]
fn explorer_email_review_waits_for_an_already_open_password_prompt() {
    let mut session = Session::new();
    let id = session.open("locked.pdf", None, Arc::new(pdf()), None).unwrap();
    session.apply(id, Edit::Protect(Protection { open_password: Some("synthetic-password".into()), ..Default::default() })).unwrap();
    let mut h = harness(None);
    h.state_mut().open_bytes("locked.pdf", None, session.save_bytes(id).unwrap().as_ref().clone()).unwrap();
    h.state_mut().receive_shell(pdfcraft_ui_egui::ShellAction::Email, &["synthetic-attachment.pdf".into()]);
    h.run_steps(3);
    assert!(h.state().password_prompt.is_some());
    assert!(h.query_by_label("Save email draft…").is_none());
    h.state_mut().submit_password(Some("synthetic-password".into()));
    // Password retries now open on a worker. Wait for the observable result rather
    // than assuming three frames are enough for decryption and document adoption.
    for _ in 0..1000 {
        h.run_steps(1);
        if h.query_by_label("Save email draft…").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(h.state().password_prompt.is_none());
    assert_eq!(h.state().session.docs().len(), 1);
    h.get_by_label("Save email draft…");
    assert_eq!(h.state().shell_email_paths.len(), 1);
}
