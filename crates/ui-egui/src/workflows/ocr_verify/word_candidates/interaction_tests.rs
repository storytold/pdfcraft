//! Offscreen inspector events, through the real Intents dispatcher. No native OCR worker.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

struct Probe {
    app: PdfCraftApp,
    applied: Vec<usize>,
    cancelled: usize,
    undone: usize,
}

fn candidate(text: &str, padding: u32) -> WordCandidate {
    WordCandidate { text: text.into(), confidence: Some(93.0), padding, source: "synthetic crop".into() }
}

fn fixture() -> PdfCraftApp {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    let bytes = app.session.create_from_text("fixture", "Synthetic inspector interaction").unwrap();
    let doc = app.session.open("inspector.pdf", None, bytes, None).unwrap();
    let settings = OcrSettings { language: "tr".into(), dpi: 240.0, ..Default::default() };
    let guard = app.session.ocr_job(doc, &[0], settings.clone()).unwrap().guard;
    let word = PlacedWord {
        text: "MAHKÜÛM.".into(),
        origin: [72.0, 700.0],
        across: [60.0, 0.0],
        up: [0.0, 12.0],
        confidence: Some(40.0),
        source: "synthetic original".into(),
    };
    app.ocr_verify.load_captured(
        doc,
        guard,
        vec![OcrPage { page: 0, words: vec![word], already_on_page: 0, skipped: None, notes: vec!["original note".into()] }],
        settings,
    );
    app.ocr_verify.open = true;
    app.ocr_verify.select((0, 0));
    app
}

fn ready(app: &mut PdfCraftApp) {
    let context = app.ocr_verify.proposal_context(&app.session).unwrap();
    app.ocr_verify.word_proposals.ready = Some(Ready { context, candidates: vec![candidate("MAHKÛM.", 8), candidate("MAHKU\u{0302}M.", 16)] });
}

fn pending(app: &mut PdfCraftApp) -> (Arc<AtomicBool>, ProposalResult) {
    let context = app.ocr_verify.proposal_context(&app.session).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(None));
    app.ocr_verify.word_proposals.pending = Some(Pending { context, cancel: cancel.clone(), result: result.clone() });
    (cancel, result)
}

fn harness(app: PdfCraftApp) -> Harness<'static, Probe> {
    let mut fonts = false;
    let mut h = Harness::builder().with_accessibility_check(false).with_size(egui::vec2(900.0, 600.0)).build_ui_state(
        move |ui, probe: &mut Probe| {
            if !fonts {
                ui.ctx().set_fonts(crate::theme::font_definitions());
                fonts = true;
                return;
            }
            let mut intents = Intents::default();
            let tokens = Tokens::get(ui.ctx());
            super::super::inspector(&mut probe.app, ui, &tokens, &mut intents);
            if let Some(index) = intents.apply_word_candidate {
                probe.applied.push(index);
            }
            probe.cancelled += usize::from(intents.cancel_word_candidates);
            probe.undone += usize::from(intents.undo);
            super::super::apply(&mut probe.app, intents);
        },
        Probe { app, applied: Vec::new(), cancelled: 0, undone: 0 },
    );
    h.run_steps(4);
    h
}

fn poll(h: &mut Harness<'_, Probe>) {
    super::super::with_state(&mut h.state_mut().app, |review, app| review.poll_word_candidates(app));
    h.run_steps(2);
}

#[test]
fn inspector_apply_second_proposal_dispatches_text_only_and_undo_restores_original() {
    let mut app = fixture();
    let before = app.ocr_verify.word((0, 0)).unwrap().clone();
    let notes = app.ocr_verify.page(0).unwrap().notes.clone();
    let doc = app.ocr_verify.doc.unwrap();
    let bytes = app.session.get(doc).unwrap().bytes.clone();
    ready(&mut app);
    let mut h = harness(app);
    assert_eq!(h.state().app.ocr_verify.word((0, 0)), Some(&before));
    assert!(h.state().app.ocr_verify.review.history.is_empty());
    // Two identically labelled controls: selecting the second must dispatch index 1.
    h.query_all_by_label("Apply").nth(1).unwrap().click();
    h.run_steps(2);
    assert_eq!(h.state().applied, vec![1]);
    let review = &h.state().app.ocr_verify;
    assert_eq!(review.word((0, 0)), Some(&PlacedWord { text: "MAHKU\u{0302}M.".into(), ..before.clone() }));
    assert_eq!(review.correction, "MAHKU\u{0302}M.");
    assert_eq!(review.page(0).unwrap().notes, notes);
    assert_eq!(review.review.history.len(), 1);
    assert!(review.word_proposals.ready.is_none());
    assert!(Arc::ptr_eq(&bytes, &h.state().app.session.get(doc).unwrap().bytes));
    h.get_by_label("Undo").click();
    h.run_steps(2);
    assert_eq!(h.state().undone, 1);
    assert_eq!(h.state().app.ocr_verify.word((0, 0)), Some(&before));
    assert_eq!(h.state().app.ocr_verify.correction, before.text);
    assert_eq!(h.get_by_role(egui::accesskit::Role::TextInput).value().as_deref(), Some(before.text.as_str()));
    assert!(h.state().app.ocr_verify.review.history.is_empty());
}

#[test]
fn inspector_undo_refreshes_selected_nfd_correction_and_preserves_noop_draft() {
    let mut app = fixture();
    let nfd = "MAHKU\u{0302}M.";
    app.ocr_verify.page_mut(0).unwrap().words.get_mut(0).unwrap().text = nfd.into();
    app.ocr_verify.select((0, 0));
    let before = app.ocr_verify.word((0, 0)).unwrap().clone();
    ready(&mut app);
    let mut h = harness(app);
    h.query_all_by_label("Apply").next().unwrap().click();
    h.run_steps(2);
    assert_eq!(h.state().app.ocr_verify.correction, "MAHK\u{00db}M.");
    h.get_by_label("Undo").click();
    h.run_steps(2);
    assert_eq!(h.state().app.ocr_verify.word((0, 0)), Some(&before));
    assert_eq!(h.state().app.ocr_verify.correction, nfd, "Undo must restore the exact NFD text in the editor as well as the model");
    assert_eq!(h.get_by_role(egui::accesskit::Role::TextInput).value().as_deref(), Some(nfd));
    assert!(h.state().app.ocr_verify.review.history.is_empty());
    edit_draft(&mut h);
    h.get_by_label("Undo").click();
    h.run_steps(2);
    assert_eq!(h.state().app.ocr_verify.correction, "taslak", "an empty history must not discard an uncommitted draft");
    assert_eq!(h.get_by_role(egui::accesskit::Role::TextInput).value().as_deref(), Some("taslak"));
    assert_eq!(h.state().app.ocr_verify.word((0, 0)), Some(&before));
    h.state_mut().app.ocr_verify.phase = Phase::Idle;
    assert!(!h.state_mut().app.ocr_verify.undo());
    assert_eq!(h.state().app.ocr_verify.correction, "taslak", "Undo outside review must preserve the uncommitted draft");
}

#[test]
fn inspector_undo_of_word_verification_preserves_uncommitted_draft() {
    let mut app = fixture();
    let before = app.ocr_verify.word((0, 0)).unwrap().clone();
    assert!(app.ocr_verify.set_word_verified((0, 0), true));
    let mut h = harness(app);
    edit_draft(&mut h);
    h.get_by_label("Undo").click();
    h.run_steps(2);
    assert_eq!(h.state().undone, 1);
    assert!(!h.state().app.ocr_verify.is_verified((0, 0)));
    assert_eq!(h.state().app.ocr_verify.correction, "taslak");
    assert_eq!(h.get_by_role(egui::accesskit::Role::TextInput).value().as_deref(), Some("taslak"));
    assert_eq!(h.state().app.ocr_verify.word((0, 0)), Some(&before));
    assert!(h.state().app.ocr_verify.review.history.is_empty());
}

#[test]
fn inspector_cancel_click_dispatches_and_late_completion_does_not_offer_apply() {
    let mut app = fixture();
    let before = app.ocr_verify.word((0, 0)).unwrap().clone();
    let (cancel, result) = pending(&mut app);
    let mut h = harness(app);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().cancelled, 1);
    assert!(cancel.load(Ordering::Acquire));
    assert!(h.state().app.ocr_verify.word_proposals.is_running());
    *result.lock().unwrap() = Some(Ok(vec![candidate("MAHKÛM.", 8)]));
    poll(&mut h);
    assert!(h.query_by_label("Apply").is_none());
    assert!(!h.state().app.ocr_verify.word_proposals.is_running());
    assert_eq!(h.state().app.ocr_verify.word((0, 0)), Some(&before));
    assert!(h.state().app.ocr_verify.review.history.is_empty());
    assert!(h.state().applied.is_empty());
}

fn edit_draft(h: &mut Harness<'_, Probe>) {
    h.get_by_role(egui::accesskit::Role::TextInput).click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("taslak".into()));
    h.run_steps(2);
    assert_eq!(h.state().app.ocr_verify.correction, "taslak");
}

#[test]
fn inspector_typing_draft_hides_ready_proposals_without_committing_a_correction() {
    let mut app = fixture();
    let before = app.ocr_verify.word((0, 0)).unwrap().clone();
    ready(&mut app);
    let mut h = harness(app);
    assert_eq!(h.query_all_by_label("Apply").count(), 2);
    edit_draft(&mut h);
    assert!(h.query_by_label("Apply").is_none());
    assert!(h.state().app.ocr_verify.word_proposals.ready.is_none());
    assert_eq!(h.state().app.ocr_verify.word((0, 0)), Some(&before));
    assert!(h.state().app.ocr_verify.review.history.is_empty());
    assert!(h.state().applied.is_empty());
}

#[test]
fn inspector_typing_draft_cancels_pending_proposal_and_discards_late_result() {
    let mut app = fixture();
    let before = app.ocr_verify.word((0, 0)).unwrap().clone();
    let (cancel, result) = pending(&mut app);
    let mut h = harness(app);
    edit_draft(&mut h);
    assert!(cancel.load(Ordering::Acquire));
    assert!(h.state().app.ocr_verify.word_proposals.is_running());
    *result.lock().unwrap() = Some(Ok(vec![candidate("MAHKÛM.", 8)]));
    poll(&mut h);
    assert!(h.query_by_label("Apply").is_none());
    assert!(!h.state().app.ocr_verify.word_proposals.is_running());
    assert_eq!(h.state().app.ocr_verify.correction, "taslak");
    assert_eq!(h.state().app.ocr_verify.word((0, 0)), Some(&before));
    assert!(h.state().app.ocr_verify.review.history.is_empty());
    assert!(h.state().applied.is_empty());
}

// The inspector harness does not enter PdfCraftApp::ui. Mirror its preference-to-TLS
// language setup, and restore the caller's language even when an assertion panics.
struct RestoreDrawingLanguage(crate::i18n::Lang);

impl RestoreDrawingLanguage {
    fn turkish_fixture() -> (PdfCraftApp, Self) {
        let previous = Self(crate::i18n::current());
        let mut app = fixture();
        app.set_option("language", "tr").unwrap();
        crate::i18n::set_current(crate::i18n::Lang::from_pref(&app.language));
        (app, previous)
    }
}

impl Drop for RestoreDrawingLanguage {
    fn drop(&mut self) {
        crate::i18n::set_current(self.0);
    }
}

#[test]
#[ignore = "requires updated embedded catalog"]
fn inspector_embedded_turkish_apply_preserves_nfd_metadata_and_undo() {
    let (mut app, _language) = RestoreDrawingLanguage::turkish_fixture();
    let before = app.ocr_verify.word((0, 0)).unwrap().clone();
    let notes = app.ocr_verify.page(0).unwrap().notes.clone();
    let doc = app.ocr_verify.doc.unwrap();
    let bytes = app.session.get(doc).unwrap().bytes.clone();
    ready(&mut app);
    let mut h = harness(app);
    // Exact rendered catalog text detects the old catalog's English fallback.
    assert_eq!(h.query_all_by_label("Bu önerinin güven puanı · synthetic crop · 93%").count(), 2);
    assert!(h.query_by_label("Confidence for this suggestion · synthetic crop · 93%").is_none());
    assert_eq!(h.state().app.ocr_verify.word((0, 0)), Some(&before));
    assert!(h.state().app.ocr_verify.review.history.is_empty());
    h.query_all_by_label("Uygula").nth(1).unwrap().click();
    h.run_steps(2);
    assert_eq!(h.state().applied, vec![1]);
    let review = &h.state().app.ocr_verify;
    assert_eq!(review.word((0, 0)), Some(&PlacedWord { text: "MAHKU\u{0302}M.".into(), ..before.clone() }));
    assert_eq!(review.correction, "MAHKU\u{0302}M.");
    assert_eq!(review.page(0).unwrap().notes, notes);
    assert_eq!(review.review.history.len(), 1);
    assert!(review.word_proposals.ready.is_none());
    assert!(Arc::ptr_eq(&bytes, &h.state().app.session.get(doc).unwrap().bytes));
    h.get_by_label("Geri al").click();
    h.run_steps(2);
    assert_eq!(h.state().undone, 1);
    assert_eq!(h.state().app.ocr_verify.word((0, 0)), Some(&before));
    assert_eq!(h.state().app.ocr_verify.correction, before.text);
    assert_eq!(h.get_by_role(egui::accesskit::Role::TextInput).value().as_deref(), Some(before.text.as_str()));
    assert!(h.state().app.ocr_verify.review.history.is_empty());
}

#[test]
#[ignore = "requires updated embedded catalog"]
fn inspector_embedded_turkish_cancel_discards_late_completion() {
    let (mut app, _language) = RestoreDrawingLanguage::turkish_fixture();
    let before = app.ocr_verify.word((0, 0)).unwrap().clone();
    let (cancel, result) = pending(&mut app);
    let mut h = harness(app);
    h.get_by_label("İptal").click();
    h.run_steps(2);
    assert_eq!(h.state().cancelled, 1);
    assert!(cancel.load(Ordering::Acquire));
    assert!(h.state().app.ocr_verify.word_proposals.is_running());
    *result.lock().unwrap() = Some(Ok(vec![candidate("MAHKU\u{0302}M.", 8)]));
    poll(&mut h);
    assert!(h.query_by_label("Uygula").is_none());
    assert!(!h.state().app.ocr_verify.word_proposals.is_running());
    assert_eq!(h.state().app.ocr_verify.word((0, 0)), Some(&before));
    assert!(h.state().app.ocr_verify.review.history.is_empty());
    assert!(h.state().applied.is_empty());
}
