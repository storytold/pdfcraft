//! Tests for the local cloud placeholder (`cloud.rs`, pending craft-libs#160).
//!
//! No real backend exists in this crate; the mock [`Service`] below is the entire
//! test double. It answers on the calling thread, the same way a real backend answering
//! in-process would: a job submitted during a frame is in the channel before the next one.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use crate::PdfCraftApp;
use crate::cloud::{self, Account, CloudBackend, Entry, EntryKind, Event, Fault, Job, Post};

type Harness = egui_kittest::Harness<'static, PdfCraftApp>;

// ---------------------------------------------------------------- in-memory service

#[derive(Default)]
struct State {
    signed_in: bool,
    display: String,
}

struct Service(Arc<Mutex<State>>);

impl CloudBackend for Service {
    fn submit(&self, job: Job, post: Post) {
        let state = self.0.lock().unwrap();
        match job {
            Job::Account | Job::SignIn => {
                let event = if state.signed_in {
                    Event::Account(Ok(Account { display: state.display.clone() }))
                } else {
                    Event::Account(Err(Fault::Unauthenticated))
                };
                post.send(event);
            }
            Job::SignOut => {}
        }
    }
}

fn harness_with(state: Arc<Mutex<State>>) -> Harness {
    // egui_kittest 0.36 uses new_eframe (PdfCraftApp implements eframe::App).
    egui_kittest::Harness::new_eframe(move |_ctx| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.cloud.connect(Service(Arc::clone(&state)));
        app
    })
}

fn signed_in_harness() -> (Harness, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State { signed_in: true, display: "Ada Lovelace".to_owned() }));
    (harness_with(Arc::clone(&state)), state)
}

fn pdf_entry(name: &str) -> Entry {
    Entry { id: name.to_owned(), name: name.to_owned(), kind: EntryKind::File, etag: Some("v1".to_owned()) }
}

fn folder_entry(name: &str) -> Entry {
    Entry { id: name.to_owned(), name: name.to_owned(), kind: EntryKind::Folder, etag: None }
}

// ---------------------------------------------------------------- availability / hiding

/// Without a connected backend, the cloud UI stays hidden rather than shown disabled: the
/// Open/Save cloud commands are disabled, sharing is unavailable, and the account badge draws
/// nothing (checked by the command-enablement assertions below and the module's own doc).
#[test]
fn cloud_unavailable_and_commands_disabled_without_a_backend() {
    let app = PdfCraftApp::new();
    assert!(!cloud::available(&app), "no backend connected");
    assert!(!cloud::shares(&app), "nothing to share without a backend");
    assert!(cloud::origin(&app).is_none(), "no document has ever opened from the cloud");
    let open_cloud = pdfcraft_engine::commands::command("file.open_cloud").expect("file.open_cloud must be registered");
    let save_cloud = pdfcraft_engine::commands::command("file.save_as_cloud").expect("file.save_as_cloud must be registered");
    assert!(!app.command_enabled(open_cloud), "file.open_cloud must be disabled without a backend");
    assert!(!app.command_enabled(save_cloud), "file.save_as_cloud must be disabled without a backend");
}

/// Connecting a backend makes `available` true, which is what ungates the cloud commands (and,
/// through the same guard, the title bar's share button, account badge and presence list).
#[test]
fn connecting_a_backend_makes_cloud_available() {
    let (h, _state) = signed_in_harness();
    assert!(cloud::available(h.state()), "a backend is connected");
}

// ---------------------------------------------------------------- sign-in / sign-out

#[test]
fn sign_in_populates_the_account_through_the_mock_backend() {
    let (mut h, _state) = signed_in_harness();
    // One frame first, so `app.ctx` (set in `logic()`) is populated before `sign_in` submits.
    h.run_steps(1);
    cloud::sign_in(h.state_mut());
    h.run_steps(4);
    let account = h.state().cloud.account.clone();
    assert!(account.is_some(), "account present after a successful sign-in");
    assert_eq!(account.unwrap().display, "Ada Lovelace");
    assert!(h.state().cloud.account_fault.is_none(), "no fault alongside a successful account");
}

#[test]
fn sign_in_failure_sets_an_account_fault_and_no_account() {
    let state = Arc::new(Mutex::new(State::default()));
    let mut h = harness_with(Arc::clone(&state));
    h.run_steps(1);
    cloud::sign_in(h.state_mut());
    h.run_steps(4);
    assert!(h.state().cloud.account.is_none(), "no account after a refused sign-in");
    assert!(matches!(h.state().cloud.account_fault, Some(Fault::Unauthenticated)), "the refusal must surface as a fault");
}

#[test]
fn sign_in_sets_a_status_message_while_in_flight() {
    let (mut h, _state) = signed_in_harness();
    h.run_steps(1);
    cloud::sign_in(h.state_mut());
    // Before pump drains the channel, the in-flight status is visible.
    assert!(h.state().cloud.status.is_some(), "a status message is set as soon as sign-in is requested");
    h.run_steps(4);
    assert!(h.state().cloud.status.is_none(), "the status clears once the account event is drained");
}

/// `sign_in` is a no-op without a connected backend: nothing crashes, and no account appears.
#[test]
fn sign_in_without_a_backend_is_a_no_op() {
    let mut app = PdfCraftApp::new();
    cloud::sign_in(&mut app);
    assert!(app.cloud.account.is_none());
    assert!(app.cloud.status.is_none(), "sign_in must not set a status when there is nothing to connect to");
}

// ---------------------------------------------------------------- pure helpers

#[test]
fn opens_recognizes_only_the_extensions_pdfcraft_can_read() {
    let pdf = pdf_entry("report.pdf");
    let fdf = pdf_entry("fields.fdf");
    let docx = pdf_entry("report.docx");
    let folder = folder_entry("Reports");
    assert!(cloud::opens(&pdf), "a .pdf entry opens");
    assert!(cloud::opens(&fdf), "a .fdf entry opens");
    assert!(!cloud::opens(&docx), "a .docx entry does not open");
    assert!(cloud::opens(&folder), "a folder is always traversable");
}

#[test]
fn pdf_name_adds_the_pdf_extension_when_none_is_recognized() {
    assert_eq!(cloud::pdf_name("quarterly report"), "quarterly report.pdf");
    assert_eq!(cloud::pdf_name(""), "Untitled.pdf");
}

#[test]
fn pdf_name_keeps_a_recognized_extension_and_strips_separators() {
    assert_eq!(cloud::pdf_name("notes.fdf"), "notes.fdf");
    assert_eq!(cloud::pdf_name("a/b\\c"), "a b c.pdf");
}

#[test]
fn copies_offers_a_pdf_export() {
    let formats = cloud::copies();
    assert!(formats.iter().any(|f| f.id == "pdf"), "a PDF copy format must be offered");
}

/// The share button is a no-op while no backend supports sharing (always, today): it must not
/// panic when actually drawn, and it must report no action taken.
#[test]
fn share_button_is_a_no_op_without_sharing_support() {
    let mut app = PdfCraftApp::new();
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::font_definitions());
    let copies = cloud::copies();
    let mut clicked: Option<&'static str> = Some("not run");
    let output = ctx.run_ui(
        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))), ..Default::default() },
        |ui| {
            clicked = cloud::share_button(&mut app, ui, &copies);
        },
    );
    output.drop_without_applying_deltas();
    assert_eq!(clicked, None, "no share action without a backend");
}
