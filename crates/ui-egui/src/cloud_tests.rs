//! Tests against the real craft-ui cloud contract (storytold/craft-libs#160), wired through
//! [`crate::PdfCraftApp`]'s `CloudApp` implementation and a small in-process mock [`Service`].
//!
//! Each test connects the mock, drives real frames through `egui_kittest::Harness`, and reads
//! either the drawn tree or the app's own state. None of this mocks `CloudApp` itself: the whole
//! point is to exercise the real `open_page`, `save_page` and gating functions against the
//! application they actually run in, the same surfaces a user reaches.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use craft_cloud::{Capability, Entry, EntryKind};
use egui_kittest::kittest::Queryable;

use crate::PdfCraftApp;
use crate::cloud::{self, Account, CloudApp, CloudBackend, Event, Fault, Job, Post};

type Harness = egui_kittest::Harness<'static, PdfCraftApp>;

/// A document the engine's lenient parser accepts with no xref table: the same literal
/// `canvas::tests` already relies on for a renderable fixture, reused here as what the mock
/// backend hands back for a `Job::Read`.
const MINIMAL_PDF: &[u8] = b"%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Contents 4 0 R >> endobj
4 0 obj << /Length 35 >> stream
0 0 1 rg 10 10 30 20 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";

// ---------------------------------------------------------------- in-memory service

#[derive(Default)]
struct State {
    signed_in: bool,
    display: String,
    /// What `Job::List` answers with, keyed by path.
    folders: std::collections::HashMap<String, Vec<Entry>>,
    saved: Vec<(String, usize)>,
}

#[derive(Clone)]
struct Service(Arc<Mutex<State>>);

impl CloudBackend for Service {
    fn submit(&self, job: Job, post: Post) {
        let mut state = self.0.lock().unwrap();
        match job {
            Job::Account | Job::SignIn => {
                let event = if state.signed_in {
                    Event::Account(Ok(Account {
                        display: state.display.clone(),
                        capabilities: vec![Capability::Files, Capability::Etags],
                        provider_linked: true,
                    }))
                } else {
                    Event::Account(Err(Fault::Unauthenticated))
                };
                post.send(event);
            }
            Job::List(path) => {
                let entries = state.folders.get(&path).cloned().unwrap_or_default();
                post.send(Event::Listed { path, entries: Ok(entries) });
            }
            Job::SharedWithMe => post.send(Event::Shared(Ok(Vec::new()))),
            Job::Read { entry, into } => post.send(Event::Read { entry, into, bytes: Ok(MINIMAL_PDF.to_vec()) }),
            Job::Write { uid, entry, bytes } => {
                state.saved.push((entry.name.clone(), bytes.len()));
                post.send(Event::Saved { uid, entry: Ok(entry) });
            }
            Job::Create { uid, folder, name, bytes } => {
                state.saved.push((name.clone(), bytes.len()));
                let entry = entry_at(&folder, &name);
                post.send(Event::Saved { uid, entry: Ok(entry) });
            }
            Job::SignOut | Job::LinkStart | Job::LinkPoll(_) | Job::Conversation(_) => {}
        }
    }
}

fn harness_with(service: Service) -> Harness {
    egui_kittest::Harness::new_eframe(move |_ctx| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.cloud.connect(service.clone());
        app
    })
}

fn signed_in_harness() -> (Harness, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State { signed_in: true, display: "Ada Lovelace".to_owned(), ..State::default() }));
    (harness_with(Service(Arc::clone(&state))), state)
}

fn entry_at(folder: &str, name: &str) -> Entry {
    Entry {
        id: format!("{folder}/{name}"),
        room: format!("{folder}/{name}"),
        name: name.to_owned(),
        path: folder.to_owned(),
        kind: EntryKind::File,
        size: Some(0),
        modified: None,
        mime: None,
        etag: Some("v1".to_owned()),
        writable: true,
        shared_by: None,
    }
}

fn folder_at(folder: &str, name: &str) -> Entry {
    Entry {
        id: format!("{folder}/{name}"),
        room: format!("{folder}/{name}"),
        name: name.to_owned(),
        path: format!("{folder}/{name}"),
        kind: EntryKind::Folder,
        size: None,
        modified: None,
        mime: None,
        etag: None,
        writable: true,
        shared_by: None,
    }
}

// ---------------------------------------------------------------- F2: the pages actually render

#[test]
fn the_open_page_draws_real_content_once_a_backend_answers() {
    let (mut harness, state) = signed_in_harness();
    state.lock().unwrap().folders.insert(String::new(), vec![entry_at("", "report.pdf")]);
    harness.state_mut().execute("file.open_cloud");
    harness.run_steps(5);
    // The real open page lists what the backend answered with, not nothing (F2: the local
    // placeholder set this state and rendered no page at all).
    harness.get_by_label("report.pdf");
    assert_eq!(harness.state().cloud_page, Some(crate::CloudPage::Open), "stays on the page while browsing, not stuck and not dismissed early");
}

#[test]
fn opening_a_cloud_entry_dismisses_the_page_from_inside_open_bytes() {
    let (mut harness, state) = signed_in_harness();
    state.lock().unwrap().folders.insert(String::new(), vec![entry_at("", "report.pdf")]);
    harness.state_mut().execute("file.open_cloud");
    harness.run_steps(5);
    harness.get_by_label("report.pdf").click();
    harness.run_steps(5);
    // Dismissed because `open_bytes` ran, not because an unrelated tab appeared and the old
    // tab-count heuristic happened to match (F5): opening a second, unrelated local document
    // afterward must not be what it takes.
    assert_eq!(harness.state().cloud_page, None);
    assert_eq!(harness.state().views.len(), 1);
}

#[test]
fn the_open_page_never_gets_stuck_open_waiting_for_a_read_that_never_answers() {
    // No folder entry registered: the mock answers the listing but there is nothing to click.
    let (mut harness, _state) = signed_in_harness();
    harness.state_mut().execute("file.open_cloud");
    harness.run_steps(30);
    // Busy never latches without a read in flight (F2's probe: ten quiet frames left it stuck).
    assert!(harness.state().cloud.busy.is_none());
}

// ---------------------------------------------------------------- F9: document_name, not pdf_name

#[test]
fn save_as_names_follow_the_real_document_name_semantics() {
    let kind = PdfCraftApp::new().kind();
    // Empty stays empty, so Save As can refuse rather than invent "Untitled.pdf".
    assert_eq!(cloud::document_name("", kind), "");
    assert_eq!(cloud::document_name("   ", kind), "");
    // A non-empty stem is required: a bare extension is not a recognized one, so it gets the
    // application's own extension appended rather than being treated as already named.
    assert_eq!(cloud::document_name(".pdf", kind), ".pdf.pdf");
    // The last path segment survives a typed path; a recognized extension is kept.
    assert_eq!(cloud::document_name("a/b/report.pdf", kind), "report.pdf");
    assert_eq!(cloud::document_name(r"a\b\report.docx", kind), "report.docx");
    // An unrecognized or missing extension gets the application's own.
    assert_eq!(cloud::document_name("notes", kind), "notes.pdf");
    assert_eq!(cloud::document_name("notes.fdf", kind), "notes.fdf.pdf");
}

// ---------------------------------------------------------------- F4: opens() matches real Open

#[test]
fn cloud_open_accepts_exactly_what_file_open_accepts() {
    let kind = PdfCraftApp::new().kind();
    for name in ["report.pdf", "scan.png", "photo.jpeg", "memo.docx", "sheet.xlsx", "notes.txt"] {
        assert!(cloud::opens(&entry_at("", name), kind), "{name} should open: File ▸ Open accepts it");
    }
    // Form-data formats belong to the form-data import dialog, not general document open.
    for name in ["form.fdf", "form.xfdf"] {
        assert!(!cloud::opens(&entry_at("", name), kind), "{name} should not open from the cloud Open page");
    }
    assert!(!cloud::opens(&folder_at("", "subfolder"), kind), "a folder is never an open target");
}

// ---------------------------------------------------------------- F6: esign is its own gate

#[test]
fn esign_availability_does_not_follow_cloud_availability() {
    let (harness, _state) = signed_in_harness();
    // The mock cloud backend is connected and signed in; esign has no backend of its own.
    assert!(cloud::available(harness.state()));
    assert!(!harness.state().esign.available(), "esign must read its own seam, not cloud::available");
}

// ---------------------------------------------------------------- F3: hidden on every surface

fn hotbar_harness(app: PdfCraftApp) -> Harness {
    egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 300.0)).build_ui_state(
        |ui, app| {
            let area = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 300.0));
            crate::hotbar::show(app, area, None, ui);
        },
        app,
    )
}

fn palette_harness(app: PdfCraftApp) -> Harness {
    egui_kittest::Harness::builder().with_size(egui::vec2(900.0, 700.0)).build_ui_state(
        |ui, app| {
            crate::dialogs::palette::show(app, ui.ctx());
        },
        app,
    )
}

#[test]
fn a_pinned_cloud_command_is_not_drawn_in_the_hotbar_with_no_backend() {
    let mut app = PdfCraftApp::new();
    app.hotbar.tools = vec!["file.open_cloud".to_owned()];
    assert!(!cloud::available(&app));
    assert!(!app.command_enabled(pdfcraft_engine::commands::command("file.open_cloud").expect("registered")));
    let mut harness = hotbar_harness(app);
    harness.run();
    // Absent, not dimmed: the old gate disabled the click but still drew the slot (F3's probe:
    // "picker lists = true, enabled = false"), which reads as a broken button.
    assert!(harness.query_by_label("Open from cloud…").is_none());
}

#[test]
fn a_pinned_cloud_command_draws_once_a_backend_is_connected() {
    let state = Arc::new(Mutex::new(State { signed_in: true, display: "Ada Lovelace".to_owned(), ..State::default() }));
    let mut app = PdfCraftApp::new();
    app.run_inline = true;
    app.cloud.connect(Service(Arc::clone(&state)));
    app.hotbar.tools = vec!["file.open_cloud".to_owned()];
    assert!(cloud::available(&app));
    let mut harness = hotbar_harness(app);
    harness.run();
    assert!(harness.query_by_label("Open from cloud…").is_some());
}

#[test]
fn the_palette_does_not_list_a_cloud_command_with_no_backend() {
    let mut app = PdfCraftApp::new();
    app.palette_open = true;
    assert!(!cloud::available(&app));
    let mut harness = palette_harness(app);
    harness.run();
    assert!(harness.query_by_label_contains("Open from cloud").is_none());
}

#[test]
fn the_palette_lists_a_cloud_command_once_a_backend_is_connected() {
    let (mut harness, _state) = signed_in_harness();
    harness.state_mut().palette_open = true;
    harness.run();
    assert!(harness.query_by_label_contains("Open from cloud").is_some());
}

// ---------------------------------------------------------------- F8: both commands execute

#[test]
fn both_cloud_commands_run_end_to_end_against_the_mock() {
    let (mut harness, state) = signed_in_harness();
    state.lock().unwrap().folders.insert(String::new(), vec![entry_at("", "report.pdf")]);
    harness.state_mut().execute("file.open_cloud");
    harness.run_steps(5);
    harness.get_by_label("report.pdf").click();
    harness.run_steps(5);
    assert_eq!(harness.state().views.len(), 1);

    harness.state_mut().execute("file.save_as_cloud");
    harness.run_steps(5);
    assert_eq!(harness.state().cloud_page, Some(crate::CloudPage::SaveAs));
    // The page drew for real: its own file name field exists, and `save_name` is already filled
    // from the open document's title (document_name's own default, not an empty field asking to
    // be typed into first).
    harness.get_by_label("File name");
    assert!(!harness.state().cloud.save_name.is_empty());
    harness.get_by_label("Save to the cloud").click();
    harness.run_steps(5);
    assert!(!state.lock().unwrap().saved.is_empty(), "the save request reached the mock");
}
