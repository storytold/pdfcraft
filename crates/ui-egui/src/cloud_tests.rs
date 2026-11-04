//! Open and Save against cloud storage, driven through the real interface with
//! a service that lives inside the test.
//!
//! The service answers on the calling thread. A job posted during a frame is
//! therefore in the inbox before the next one. Everything else is the shipping
//! code: the Open page, `PdfCraftApp::run`, the conflict modal, and the PDF
//! path.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use crate::PdfCraftApp;
use crate::cloud::{self, Account, Capability, CloudApp, CloudBackend, Destination, Entry, EntryKind, Event, Fault, Job, Post};

type Harness = egui_kittest::Harness<'static, PdfCraftApp>;

// ---------------------------------------------------------------- in-memory service

#[derive(Default)]
struct State {
    files: Vec<(Entry, Vec<u8>)>,
    shared: Vec<(Entry, Vec<u8>)>,
    capabilities: Vec<Capability>,
    linked: bool,
    writes: Vec<(String, Option<String>, usize)>,
    creates: Vec<(String, String, usize)>,
}

impl State {
    fn account(&self) -> Result<Account, Fault> {
        if !self.linked {
            return Err(Fault::NotLinked);
        }
        Ok(Account { display: "Ada Lovelace".to_owned(), capabilities: self.capabilities.clone(), provider_linked: true })
    }

    fn find(&self, id: &str) -> Option<&(Entry, Vec<u8>)> {
        self.files.iter().chain(self.shared.iter()).find(|(e, _)| e.id == id)
    }

    fn overwrite(&mut self, id: &str) {
        if let Some(slot) = self.files.iter_mut().chain(self.shared.iter_mut()).find(|(e, _)| e.id == id) {
            slot.0.etag = Some(format!("{}-next", slot.0.etag.clone().unwrap_or_default()));
        }
    }

    fn answer(&mut self, job: Job) -> Vec<Event> {
        if matches!(job, Job::SignOut) {
            self.linked = false;
            return Vec::new();
        }
        if !self.linked && !matches!(job, Job::LinkStart | Job::LinkPoll(_)) {
            return vec![match job {
                Job::Account => Event::Account(Err(Fault::NotLinked)),
                Job::List(path) => Event::Listed { path, entries: Err(Fault::NotLinked) },
                Job::SharedWithMe => Event::Shared(Err(Fault::NotLinked)),
                Job::Read { entry, into, .. } => Event::Read { entry, into, bytes: Err(Fault::NotLinked) },
                Job::Write { uid, .. } | Job::Create { uid, .. } => Event::Saved { uid, entry: Err(Fault::NotLinked) },
                Job::LinkStart | Job::LinkPoll(_) => Event::Linked(Ok(false)),
                Job::Conversation(_) => Event::Conversation(Err(Fault::NotLinked)),
                Job::SignOut => return Vec::new(),
            }];
        }
        match job {
            Job::SignOut => Vec::new(),
            Job::Conversation(_) => {
                if !self.capabilities.contains(&Capability::Conversation) {
                    return vec![Event::Conversation(Err(Fault::Refused("this provider has nowhere to talk".to_owned())))];
                }
                let room = craft_cloud::Conversation { url: "https://cloud.example/call/room".to_owned(), name: "Talk".to_owned() };
                vec![Event::Conversation(Ok(room))]
            }
            Job::Account => vec![Event::Account(self.account())],
            Job::LinkStart => vec![Event::Browser { url: "https://cloud.example/link/abc".to_owned(), handle: Some("abc".to_owned()) }],
            Job::LinkPoll(_) => vec![Event::Linked(Ok(self.linked))],
            Job::List(path) => {
                let entries = self.files.iter().filter(|(e, _)| parent(&e.path) == path).map(|(e, _)| e.clone()).collect();
                vec![Event::Listed { path, entries: Ok(entries) }]
            }
            Job::SharedWithMe => {
                if !self.capabilities.contains(&Capability::SharedWithMe) {
                    return vec![Event::Shared(Err(Fault::Refused("no incoming shares".to_owned())))];
                }
                vec![Event::Shared(Ok(self.shared.iter().map(|(e, _)| e.clone()).collect()))]
            }
            Job::Read { entry, into } => {
                let bytes = match self.find(&entry.id) {
                    Some((_, bytes)) => Ok(bytes.clone()),
                    None => Err(Fault::Refused("no such file".to_owned())),
                };
                vec![Event::Read { entry, into, bytes }]
            }
            Job::Write { uid, entry, bytes } => {
                self.writes.push((entry.id.clone(), entry.etag.clone(), bytes.len()));
                let stored_etag = self.find(&entry.id).and_then(|(e, _)| e.etag.clone());
                let answer = match stored_etag {
                    None => Err(Fault::Refused("no such file".to_owned())),
                    Some(ref stored) if *stored != entry.etag.as_deref().unwrap_or("") => Err(Fault::Conflict),
                    Some(_) => {
                        let slot = self.files.iter_mut().chain(self.shared.iter_mut()).find(|(e, _)| e.id == entry.id);
                        match slot {
                            Some(slot) => {
                                slot.0.etag = Some(format!("{}+", slot.0.etag.clone().unwrap_or_default()));
                                slot.0.size = Some(bytes.len() as u64);
                                slot.1 = bytes;
                                Ok(slot.0.clone())
                            }
                            None => Err(Fault::Refused("no such file".to_owned())),
                        }
                    }
                };
                vec![Event::Saved { uid, entry: answer }]
            }
            Job::Create { uid, folder, name, bytes } => {
                self.creates.push((folder.clone(), name.clone(), bytes.len()));
                let path = format!("{}/{name}", folder.trim_end_matches('/'));
                let entry = pdf_entry(&path);
                self.files.push((entry.clone(), bytes));
                vec![Event::Saved { uid, entry: Ok(entry) }]
            }
        }
    }
}

fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => String::new(),
        Some(cut) => path[..cut].to_owned(),
    }
}

fn pdf_entry(path: &str) -> Entry {
    Entry {
        id: path.to_owned(),
        room: format!("room:{path}"),
        name: path.rsplit('/').next().unwrap_or(path).to_owned(),
        path: path.to_owned(),
        kind: EntryKind::File,
        size: Some(4096),
        modified: Some("2026-10-01T09:00:00Z".to_owned()),
        mime: Some("application/pdf".to_owned()),
        etag: Some("v1".to_owned()),
        writable: true,
        shared_by: None,
    }
}

fn folder_entry(path: &str) -> Entry {
    Entry { kind: EntryKind::Folder, size: None, etag: None, mime: None, ..pdf_entry(path) }
}

struct Service(Arc<Mutex<State>>);

impl CloudBackend for Service {
    fn submit(&self, job: Job, post: Post) {
        let events = self.0.lock().unwrap().answer(job);
        for event in events {
            post.send(event);
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

fn linked_harness() -> (Harness, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State { linked: true, capabilities: vec![Capability::Files, Capability::Etags], ..Default::default() }));
    (harness_with(Arc::clone(&state)), state)
}

// ---------------------------------------------------------------- tests

#[test]
fn account_offers_link_before_sign_in() {
    let state = Arc::new(Mutex::new(State::default()));
    let mut h = harness_with(Arc::clone(&state));
    cloud::start(h.state_mut());
    h.run_steps(4);
    let cloud = &h.state().cloud;
    assert!(cloud.account.is_none(), "no account before linking");
    assert!(cloud.account_fault.is_none(), "NotLinked is an actionable state, not an error");
    assert!(matches!(cloud.link, Some(cloud::Link::Offered)), "link offer expected while unlinked; got: {:?}", cloud.link);
}

#[test]
fn linked_account_reported_after_start() {
    let (mut h, _state) = linked_harness();
    cloud::start(h.state_mut());
    h.run_steps(4);
    let account = h.state().cloud.account.clone();
    assert!(account.is_some(), "account present after linking");
    assert_eq!(account.unwrap().display, "Ada Lovelace");
}

#[test]
fn account_fetched_by_update_loop_without_explicit_start() {
    // start() is called from update() since the fix; verify the account
    // arrives without a hand-written start() call. This is the browser
    // case: the session rides the same-origin cookie set at the root
    // sign-in, and the user never has to open File > Open for the
    // title-bar controls to reflect it.
    let (mut h, _state) = linked_harness();
    // Deliberately no cloud::start() here. The update loop provides it.
    h.run_steps(4);
    let account = h.state().cloud.account.clone();
    assert!(account.is_some(), "update loop must bootstrap the account without an explicit start()");
    assert_eq!(account.unwrap().display, "Ada Lovelace");
}

#[test]
fn sign_out_clears_account() {
    let (mut h, _state) = linked_harness();
    cloud::start(h.state_mut());
    h.run_steps(4);
    assert!(h.state().cloud.account.is_some(), "signed in before sign-out");
    cloud::sign_out(h.state_mut());
    h.run_steps(4);
    assert!(h.state().cloud.account.is_none(), "account cleared after sign-out");
}

#[test]
fn open_page_lists_root_when_linked() {
    let (mut h, state) = linked_harness();
    {
        let mut s = state.lock().unwrap();
        s.files.push((pdf_entry("/docs/report.pdf"), vec![]));
        s.files.push((folder_entry("/docs"), vec![]));
    }
    cloud::start(h.state_mut());
    h.run_steps(8);
    // The root listing was fetched: account is present and the mine folder
    // reports that it has loaded at least once.
    assert!(h.state().cloud.account.is_some(), "account fetched after start");
    assert!(h.state().cloud.mine.loaded, "root folder listing arrived");
}

#[test]
fn sign_out_does_not_leave_unauthenticated_fault() {
    // After sign_out(), craft-libs resets asked_account to false so the next
    // start() re-submits Job::Account.  The in-memory service then returns
    // NotLinked (it drops the session on SignOut).  The important assertion is
    // that account_fault is NOT Unauthenticated: Unauthenticated is what the
    // browser transport maps to a login redirect, and seeing it here would
    // mean a sign-out loop in the browser.  NotLinked is handled differently
    // (it offers a link prompt rather than a redirect).
    let (mut h, _state) = linked_harness();
    h.run_steps(4);
    assert!(h.state().cloud.account.is_some(), "signed in before sign-out");

    cloud::sign_out(h.state_mut());
    h.run_steps(4);

    assert!(h.state().cloud.account.is_none(), "account cleared after sign-out");
    assert!(
        !matches!(h.state().cloud.account_fault, Some(Fault::Unauthenticated)),
        "Unauthenticated fault after sign-out would trigger a login redirect loop; got: {:?}",
        h.state().cloud.account_fault,
    );
}

#[test]
fn writes_as_rejects_non_pdf_extension() {
    let dest = Destination { name: "out.xlsx", mime: None };
    let mut app = PdfCraftApp::new();
    let result = app.encode(0, dest);
    assert!(result.is_err(), "must refuse to encode as .xlsx");
    let msg = result.unwrap_err();
    assert!(msg.contains("PdfCraft saves PDFs"), "error mentions the format constraint: {msg}");
}

#[test]
fn writes_as_accepts_bare_name_as_pdf() {
    // A bare name (no extension) must not be rejected by the format check.
    // The engine error (no such doc) comes first, but if the format check
    // rejects it, the test knows the wrong thing failed.
    let dest = Destination { name: "untitled", mime: None };
    let mut app = PdfCraftApp::new();
    // This will fail with an engine error (no doc 0), not a format error.
    let result = app.encode(0, dest);
    assert!(result.is_err());
    let msg = result.unwrap_err();
    // Must not say "PdfCraft saves PDFs" for a bare name.
    assert!(!msg.contains("PdfCraft saves PDFs"), "bare name must pass the format check; got: {msg}");
}

#[test]
fn conflict_is_stored_after_a_refused_conditional_write() {
    let entry = pdf_entry("/docs/shared.pdf");
    let state = Arc::new(Mutex::new(State {
        linked: true,
        capabilities: vec![Capability::Files, Capability::Etags],
        // The file must have real PDF bytes so open_bytes can parse it.
        files: vec![(entry.clone(), minimal_pdf())],
        ..Default::default()
    }));
    let mut h = harness_with(Arc::clone(&state));
    h.run_steps(4);
    // Open the document from the cloud so an origin with the v1 etag is recorded.
    cloud::open_entry(h.state_mut(), entry.clone(), None);
    h.run_steps(8);
    assert!(h.state().active.is_some(), "document must be open before testing the save conflict");
    // Another client overwrites the file so the etag on the server advances.
    state.lock().unwrap().overwrite("/docs/shared.pdf");
    // Trigger a cloud save. The etag no longer matches → the server returns Conflict.
    cloud::save(h.state_mut()).expect("save submission must not fail synchronously");
    h.run_steps(8);
    let writes = state.lock().unwrap().writes.clone();
    // Exactly one write attempt (the conflict must not be retried automatically).
    assert_eq!(writes.len(), 1, "conflict must not be retried automatically");
    assert!(h.state().cloud.conflict.is_some(), "conflict state set after a refused conditional write");
}

#[test]
fn presence_bar_holds_peers_that_were_set() {
    use craft_ui::presence::Peer;
    let (mut h, _state) = linked_harness();
    let peers = vec![Peer::new("peer-1".to_owned(), "Babbage"), Peer::new("peer-2".to_owned(), "Turing")];
    h.state_mut().presence.set(peers);
    h.run_steps(2);
    assert_eq!(h.state().presence.peers().len(), 2);
}

#[test]
fn document_kind_only_opens_pdfs() {
    use crate::cloud::{DOCUMENT, opens};
    let pdf = pdf_entry("/test.pdf");
    let docx = Entry { mime: Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_owned()), ..pdf_entry("/test.docx") };
    let folder = folder_entry("/folder");
    assert!(opens(&pdf), "a .pdf entry opens");
    assert!(!opens(&docx), "a .docx entry does not open");
    assert!(!opens(&folder), "a folder does not open");
    // DOCUMENT noun for user messages.
    assert_eq!(DOCUMENT.noun, "PDF");
    assert_eq!(DOCUMENT.default_extension, "pdf");
}

// ---------------------------------------------------------------- UI-path / accessibility tests

/// Build a minimal but legal one-page PDF entirely in memory.
/// Mirrors the fixture builder in open_worker_tests so the format is known-good.
fn minimal_pdf() -> Vec<u8> {
    let body = "BT /F1 24 Tf 20 150 Td (Test) Tj ET";
    let objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    let n = objects.len() + 1;
    bytes.extend_from_slice(format!("trailer\n<< /Size {n} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

/// Open a minimal PDF into the harness and return its document uid.
fn open_minimal(h: &mut Harness) -> u64 {
    let bytes = minimal_pdf();
    h.state_mut().open_bytes("test.pdf", None, bytes).expect("open minimal PDF");
    h.run_steps(2);
    h.state().active_ids().expect("document must be active after open").1.0
}

/// Render one frame using an egui Context and collect all non-empty text galley strings.
/// This is the same technique the home-tab test uses to prove labels are visible.
///
/// `ctx.run_ui` needs `FnMut`. Both the `draw` closure and the mutable app reference are
/// wrapped in `Option` and moved into the `FnMut` closure; `.take()` consumes each exactly
/// once on the first (and only) frame — safe with no `unsafe` required.
fn shapes_from_page(app: &mut PdfCraftApp, size: egui::Vec2, draw: impl FnOnce(&mut PdfCraftApp, &mut egui::Ui) + 'static) -> Vec<String> {
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::font_definitions());
    // Both the `FnOnce` draw closure and the `&mut app` are moved into Options.
    // Inside the `FnMut`, `.take()` extracts them at most once; subsequent frames
    // (there are none in practice) are no-ops.  No unsafe needed.
    type DrawOnce = Box<dyn FnOnce(&mut PdfCraftApp, &mut egui::Ui)>;
    let mut draw_opt: Option<DrawOnce> = Some(Box::new(draw));
    let mut app_opt: Option<&mut PdfCraftApp> = Some(app);
    let output = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() }, |ui| {
        if let (Some(f), Some(a)) = (draw_opt.take(), app_opt.take()) {
            f(a, ui);
        }
    });
    let texts: Vec<String> = output
        .shapes
        .iter()
        .filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t.galley.job.text.clone()) } else { None })
        .filter(|s| !s.is_empty())
        .collect();
    output.drop_without_applying_deltas();
    texts
}

// ---- label visibility ---------------------------------------------------

/// The cloud Open page shell must render a visible "Open from cloud" heading and a "Back"
/// button. This is the accessibility regression guard: heading + navigation control must both
/// be text shapes in the render tree, not invisible or zero-size.
#[test]
fn open_from_cloud_page_heading_and_back_button_are_visible() {
    let (mut h, _state) = linked_harness();
    h.run_steps(4);
    let app = h.state_mut();
    app.cloud_page = Some(crate::CloudPage::Open);
    let labels = shapes_from_page(app, egui::vec2(1200.0, 800.0), |app, ui| {
        crate::cloud_page_shell(app, ui, crate::cloud::open_page);
    });
    assert!(labels.iter().any(|l| l.contains("Open from cloud")), "cloud Open page must render a visible heading; rendered: {labels:?}",);
    assert!(labels.iter().any(|l| l.contains("Back")), "Back button must render visible text for screen-reader navigation; rendered: {labels:?}",);
}

/// The cloud Save As page shell must render a visible heading and "Back" button.
#[test]
fn save_as_cloud_page_heading_and_back_button_are_visible() {
    let (mut h, _state) = linked_harness();
    h.run_steps(4);
    let app = h.state_mut();
    app.cloud_page = Some(crate::CloudPage::SaveAs);
    let labels = shapes_from_page(app, egui::vec2(1200.0, 800.0), |app, ui| {
        crate::cloud_page_shell(app, ui, crate::cloud::save_page);
    });
    assert!(labels.iter().any(|l| l.contains("Save as to cloud")), "cloud Save As page must render a visible heading; rendered: {labels:?}",);
    assert!(labels.iter().any(|l| l.contains("Back")), "Back button must render visible text for screen-reader navigation; rendered: {labels:?}",);
}

/// The "Open from cloud…" command must be enabled when a cloud service is connected.
/// When enabled, the tab strip popup includes "Open from cloud…" and the File menu shows it.
#[test]
fn open_from_cloud_command_enabled_when_service_connected() {
    let (mut h, _state) = linked_harness();
    h.run_steps(4);
    let spec = pdfcraft_engine::commands::command("file.open_cloud").expect("file.open_cloud command must be registered");
    assert!(h.state().command_enabled(spec), "file.open_cloud must be enabled when cloud service is connected",);
}

/// The cloud commands are disabled and absent from the File menu when no service is configured.
#[test]
fn cloud_commands_disabled_without_service() {
    let app = PdfCraftApp::new();
    // No backend connected — cloud not available.
    let open_cloud = pdfcraft_engine::commands::command("file.open_cloud").unwrap();
    let save_cloud = pdfcraft_engine::commands::command("file.save_as_cloud").unwrap();
    assert!(!app.command_enabled(open_cloud), "file.open_cloud must be disabled without a service");
    assert!(!app.command_enabled(save_cloud), "file.save_as_cloud must be disabled without a service");
}

/// The cloud Save As command is available only when a document is open AND the service is connected.
#[test]
fn save_as_cloud_requires_document_and_service() {
    let (mut h, _state) = linked_harness();
    h.run_steps(4);
    let spec = pdfcraft_engine::commands::command("file.save_as_cloud").unwrap();
    // No document open yet: disabled even though service is connected.
    assert!(!h.state().command_enabled(spec), "file.save_as_cloud requires an open document");
    // Open a document.
    open_minimal(&mut h);
    assert!(h.state().command_enabled(spec), "file.save_as_cloud must be enabled with a document and service");
}

// ---- UI-path: Open from cloud -------------------------------------------

/// Cloud Open page dismisses itself once a document is successfully opened.
/// The test backend answers Job::Read immediately, so the open completes within run_steps.
#[test]
fn open_from_cloud_page_dismisses_after_successful_open() {
    let pdf = minimal_pdf();
    let entry = pdf_entry("/docs/report.pdf");
    let state = Arc::new(Mutex::new(State {
        linked: true,
        capabilities: vec![Capability::Files, Capability::Etags],
        files: vec![(entry.clone(), pdf)],
        ..Default::default()
    }));
    let mut h = harness_with(Arc::clone(&state));
    h.run_steps(4);
    // Activate the Open page and record how many docs are open.
    let doc_count_before = h.state().views.len();
    h.state_mut().cloud_page_doc_count = doc_count_before;
    h.state_mut().cloud_page = Some(crate::CloudPage::Open);
    h.run_steps(2);
    // Trigger an open through the cloud API (same path as open_page's pick handler).
    cloud::open_entry(h.state_mut(), entry, None);
    // Allow pump to drain the Read event and open the document.
    h.run_steps(8);
    assert!(h.state().views.len() > doc_count_before, "document must have opened");
    assert!(h.state().cloud_page.is_none(), "cloud Open page must dismiss after successful open");
    assert!(h.state().active.is_some(), "opened document must be active");
    // The cloud origin is recorded so file.save knows where to write back.
    let uid = h.state().active_ids().unwrap().1.0;
    assert!(h.state().cloud.origins.contains_key(&uid), "origin must be recorded for the opened document");
}

// ---- UI-path: Save to cloud ---------------------------------------------

/// file.save writes to the cloud origin (with etag) when the document came from the cloud.
/// Local Save is untouched for documents that did not come from the cloud.
#[test]
fn file_save_writes_to_cloud_when_document_has_cloud_origin() {
    let pdf = minimal_pdf();
    let entry = pdf_entry("/docs/report.pdf");
    let state = Arc::new(Mutex::new(State {
        linked: true,
        capabilities: vec![Capability::Files, Capability::Etags],
        files: vec![(entry.clone(), pdf)],
        ..Default::default()
    }));
    let mut h = harness_with(Arc::clone(&state));
    h.run_steps(4);
    // Open through the cloud path so an origin is recorded.
    cloud::open_entry(h.state_mut(), entry, None);
    h.run_steps(8);
    assert!(h.state().active.is_some(), "document must be open after cloud open");
    let writes_before = state.lock().unwrap().writes.len();
    // Trigger Save through the command (mode bar Save button path).
    h.state_mut().execute("file.save");
    h.run_steps(8);
    let writes_after = state.lock().unwrap().writes.clone();
    assert!(writes_after.len() > writes_before, "file.save must have written to the cloud");
    // The write must carry the etag so the server can enforce optimistic locking.
    let (_, etag, _) = writes_after.last().unwrap();
    assert!(etag.is_some(), "cloud write must include an etag for conditional update");
}

/// file.save falls back to local save for a document with no cloud origin.
/// Verified by checking that the cloud write count does not increase.
#[test]
fn file_save_stays_local_when_document_has_no_cloud_origin() {
    let (mut h, state) = linked_harness();
    h.run_steps(4);
    // No cloud open → no origin.
    assert!(h.state().cloud.origins.is_empty(), "no origins before any cloud open");
    // Even if the command runs, nothing is written to the cloud.
    let writes_before = state.lock().unwrap().writes.len();
    h.state_mut().execute("file.save");
    h.run_steps(4);
    let writes_after = state.lock().unwrap().writes.len();
    assert_eq!(writes_before, writes_after, "file.save must not write to cloud for a local document");
}

// ---- UI-path: Save As to cloud ------------------------------------------

/// file.save_as_cloud sets cloud_page to SaveAs.
#[test]
fn file_save_as_cloud_command_opens_save_page() {
    let (mut h, _state) = linked_harness();
    h.run_steps(4);
    open_minimal(&mut h);
    h.state_mut().execute("file.save_as_cloud");
    assert_eq!(h.state().cloud_page, Some(crate::CloudPage::SaveAs), "file.save_as_cloud must open the cloud Save As page");
}

/// Cloud Save As creates a new file in the chosen folder.
#[test]
fn cloud_save_as_creates_file_in_cloud() {
    let pdf = minimal_pdf();
    let folder = folder_entry("/projects");
    let state = Arc::new(Mutex::new(State {
        linked: true,
        capabilities: vec![Capability::Files, Capability::Etags],
        files: vec![(folder, pdf)],
        ..Default::default()
    }));
    let mut h = harness_with(Arc::clone(&state));
    h.run_steps(4);
    open_minimal(&mut h);
    // Show the Save As page and prime the name field.
    h.state_mut().cloud_page = Some(crate::CloudPage::SaveAs);
    h.state_mut().cloud.save_name = "quarterly-report.pdf".to_owned();
    // Invoke save_as directly (same path as the Save button on the page).
    let result = cloud::save_as(h.state_mut(), "/projects", "quarterly-report.pdf");
    assert!(result.is_ok(), "save_as must succeed: {result:?}");
    h.run_steps(8);
    let creates = state.lock().unwrap().creates.clone();
    assert!(!creates.is_empty(), "a new cloud file must have been created");
    let (folder_path, name, _) = creates.first().unwrap();
    assert_eq!(folder_path, "/projects");
    assert_eq!(name, "quarterly-report.pdf");
}

// ---- go_to_save_as (conflict modal → Save As page) ---------------------

/// go_to_save_as sets cloud_page to SaveAs so the conflict modal can redirect to the cloud
/// Save As page. This matches the WordCraft/DeckCraft backstage pattern.
#[test]
fn go_to_save_as_sets_cloud_save_as_page() {
    let (mut h, _state) = linked_harness();
    h.run_steps(4);
    use cloud::CloudApp;
    h.state_mut().go_to_save_as();
    assert_eq!(h.state().cloud_page, Some(crate::CloudPage::SaveAs), "go_to_save_as must open the cloud Save As page");
}
