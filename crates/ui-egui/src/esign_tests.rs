//! Tests against the real craft-ui esign contract (storytold/craft-libs#661), wired through
//! [`crate::PdfCraftApp`]'s `esign` field and a small in-process mock [`Service`].
//!
//! None of this mocks `EsignDialog` or `EsignBackend` away: each test connects the mock to the
//! real dialog and drives real frames through `egui_kittest::Harness`, the same pattern
//! `cloud_tests.rs` uses for the cloud feature.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use craft_esign::EsignCapability;
use craft_ui::esign::{Answer, EsignBackend, Replier, Request};
use egui_kittest::kittest::Queryable;

use crate::PdfCraftApp;

type Harness = egui_kittest::Harness<'static, PdfCraftApp>;

#[derive(Default)]
struct State {
    linked: bool,
}

#[derive(Clone)]
struct Service(Arc<Mutex<State>>);

impl EsignBackend for Service {
    fn submit(&self, request: Request, reply: Replier) {
        let state = self.0.lock().unwrap();
        match request {
            Request::Capabilities => reply.answer(Answer::Capabilities {
                capabilities: vec![EsignCapability::Envelopes],
                responsible_party: "Stub Signing".to_owned(),
                linked: state.linked,
            }),
            Request::Lookup { .. } => reply.answer(Answer::Lookup(Vec::new())),
            Request::Create { .. } | Request::Status { .. } | Request::Completed { .. } | Request::Void { .. } | Request::Remind { .. } => {
                reply.refuse("not wired in this test");
            }
        }
    }
}

fn harness_with(service: Service) -> Harness {
    egui_kittest::Harness::new_eframe(move |_ctx| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.esign.set_backend(service.clone());
        app
    })
}

// ---------------------------------------------------------------- independence from cloud

#[test]
fn esign_hides_with_no_backend_connected() {
    let mut harness: Harness = egui_kittest::Harness::new_eframe(|_ctx| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app
    });
    harness.run_steps(2);
    assert!(!harness.state().esign.available());
    assert!(!harness.state().cloud.available());
}

#[test]
fn esign_becomes_available_from_its_own_backend_alone() {
    let state = Arc::new(Mutex::new(State { linked: true }));
    let mut harness = harness_with(Service(Arc::clone(&state)));
    harness.run_steps(2);
    // The capability probe answers inline, same thread, same frame: one connected backend is
    // enough. Nothing here ever connects a cloud backend (F6 in the review): esign's own
    // availability must not depend on crate::cloud::available.
    assert!(harness.state().esign.available());
    assert!(!harness.state().cloud.available());
}

// ---------------------------------------------------------------- the title bar's own gate

#[test]
fn the_title_bar_offers_nothing_with_no_backend() {
    let mut harness: Harness = egui_kittest::Harness::new_eframe(|_ctx| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app
    });
    harness.run_steps(3);
    assert!(harness.query_by_label("Request signatures").is_none());
}

#[test]
fn the_title_bar_offers_request_signatures_once_available() {
    let state = Arc::new(Mutex::new(State { linked: true }));
    let mut harness = harness_with(Service(Arc::clone(&state)));
    harness.run_steps(3);
    harness.get_by_label("Request signatures");
}

// ---------------------------------------------------------------- pump wiring

#[test]
fn pump_runs_every_frame_whether_or_not_the_dialog_is_open() {
    let state = Arc::new(Mutex::new(State { linked: true }));
    let mut harness = harness_with(Service(Arc::clone(&state)));
    assert!(!harness.state().esign.is_open());
    harness.run_steps(3);
    // The capability probe ran with the dialog closed the whole time: `show` (and this crate's
    // own `pump` wrapper around it) must drive it regardless of `is_open`, the same way the
    // cloud feature's account probe runs with no cloud page ever shown.
    assert!(harness.state().esign.available());
    assert!(!harness.state().esign.is_open());
}

#[test]
fn opening_the_dialog_uses_the_real_craft_cloud_identity() {
    let state = Arc::new(Mutex::new(State { linked: true }));
    let mut harness = harness_with(Service(Arc::clone(&state)));
    harness.run_steps(2);
    let sender = craft_cloud::Identity { id: "a".to_owned(), display: "Ada Lovelace".to_owned(), email: None, avatar: None };
    harness.state_mut().esign.open("doc-1".to_owned(), "report.pdf".to_owned(), None, sender);
    harness.run_steps(2);
    assert!(harness.state().esign.is_open());
}
