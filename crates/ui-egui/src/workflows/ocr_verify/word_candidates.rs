//! Explicit selected-word proposals. Completing a worker never edits review words.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::{DocId, Intents, OcrGuard, OcrPage, OcrSettings, OcrVerify, PdfCraftApp, Phase, PlacedWord, Session, Tokens};
use pdfcraft_engine::ocr::WordCandidate;

#[derive(Clone)]
struct Context {
    doc: DocId,
    guard: OcrGuard,
    key: (usize, usize),
    word: PlacedWord,
    settings: OcrSettings,
    correction: String,
}

impl Context {
    fn current(&self, review: &OcrVerify, session: &Session) -> bool {
        review.open
            && review.phase == Phase::ReviewReady
            && review.doc == Some(self.doc)
            && review.selected == Some(self.key)
            && review.current == self.key.0
            && !review.is_removed(self.key)
            && !review.review.applied_pages.contains(&self.key.0)
            && review.word(self.key) == Some(&self.word)
            && review.captured_settings.as_ref() == Some(&self.settings)
            && review.correction == self.correction
            && review.reread.is_none()
            && session.stale_reason(&self.guard).is_none()
    }
}

type ProposalResult = Arc<Mutex<Option<Result<Vec<WordCandidate>, String>>>>;

struct Pending {
    context: Context,
    cancel: Arc<AtomicBool>,
    result: ProposalResult,
}

struct Ready {
    context: Context,
    candidates: Vec<WordCandidate>,
}

#[derive(Default)]
pub(super) struct Proposals {
    pending: Option<Pending>,
    ready: Option<Ready>,
}

impl Proposals {
    pub(super) fn is_running(&self) -> bool {
        self.pending.is_some()
    }
}

impl OcrVerify {
    pub(super) fn apply_word_candidate_intents(&mut self, app: &mut PdfCraftApp, cancel: bool, request: bool, apply: Option<usize>) {
        if cancel {
            self.invalidate_word_candidates();
        }
        if request {
            self.start_word_candidates(app);
        }
        if let Some(index) = apply {
            self.apply_word_candidate(&app.session, index);
        }
    }

    /// A caller that retained the actual settings may enable proposals; a plain `load`
    /// intentionally leaves them disabled instead of inventing a recognition profile.
    pub(crate) fn load_captured(&mut self, doc: DocId, guard: OcrGuard, found: Vec<OcrPage>, settings: OcrSettings) {
        self.dictionary_language = settings.language.clone();
        self.load(doc, guard, found);
        self.captured_settings = Some(settings);
    }

    pub(super) fn invalidate_word_candidates(&mut self) {
        self.word_proposals.ready = None;
        if let Some(pending) = &self.word_proposals.pending {
            pending.cancel.store(true, Ordering::Release);
        }
        // Retain the cancelled worker until it lands: repeated clicks cannot create
        // parallel native work, including after a new review replaces this one.
    }

    fn proposal_context(&self, session: &Session) -> Result<Context, String> {
        let doc = self.doc.ok_or(tl!("the document is no longer open"))?;
        let guard = self.guard.clone().ok_or(tl!("the recognition review is unavailable"))?;
        let key = self.selected.ok_or(tl!("select a word to inspect it"))?;
        let word = self.word(key).cloned().ok_or(tl!("the selected word is gone; choose another"))?;
        let settings = self.captured_settings.clone().ok_or(tl!("the original recognition settings are unavailable; run OCR again"))?;
        let context = Context { doc, guard, key, word, settings, correction: self.correction.clone() };
        if !context.current(self, session) {
            return Err(session.stale_reason(&context.guard).unwrap_or_else(|| tl!("the selected word no longer belongs to this review").into()));
        }
        Ok(context)
    }

    pub(super) fn start_word_candidates(&mut self, app: &mut PdfCraftApp) {
        if self.word_proposals.is_running() || self.reread.is_some() {
            return;
        }
        let context = match self.proposal_context(&app.session) {
            Ok(context) => context,
            Err(why) => {
                self.message = why;
                return;
            }
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = context;
            self.message = tl!("Word suggestions require the desktop recognizer").into();
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(job) = app.session.ocr_job(context.doc, &[context.key.0], context.settings.clone()) else {
                self.message = tl!("the document could not be read").into();
                return;
            };
            self.invalidate_word_candidates();
            let cancel = Arc::new(AtomicBool::new(false));
            let result = Arc::new(Mutex::new(None));
            let output = result.clone();
            let cancelled = cancel.clone();
            let word = context.word.clone();
            let page = context.key.0;
            let work = move || {
                let found = pdfcraft_engine::guard(|| {
                    if cancelled.load(Ordering::Acquire) {
                        return Ok(Vec::new());
                    }
                    let recognizers = pdfcraft_engine::ocr::recognizers(&job.settings)?;
                    if cancelled.load(Ordering::Acquire) {
                        return Ok(Vec::new());
                    }
                    // The bounded engine method owns exactly two crop reads. It has no
                    // cancellation parameter; cancellation always discards its result.
                    job.word_candidates(page, &word, &recognizers)
                })
                .unwrap_or_else(Err);
                if let Ok(mut slot) = output.lock() {
                    *slot = Some(found);
                }
            };
            self.word_proposals.pending = Some(Pending { context, cancel, result });
            self.message = tl!("Recognize text").to_string();
            if app.run_inline {
                work();
            } else if let Err(error) = std::thread::Builder::new().name("pdfcraft-ocr-word-proposals".into()).spawn(work) {
                self.word_proposals.pending = None;
                self.message = error.to_string();
            }
        }
    }

    pub(super) fn poll_word_candidates(&mut self, app: &mut PdfCraftApp) {
        if self.word_proposals.ready.as_ref().is_some_and(|ready| !ready.context.current(self, &app.session)) {
            self.invalidate_word_candidates();
        }
        let Some(pending) = &self.word_proposals.pending else { return };
        if !pending.context.current(self, &app.session) {
            pending.cancel.store(true, Ordering::Release);
        }
        let output = pending.result.clone();
        let result = match output.lock() {
            Ok(mut slot) => slot.take(),
            Err(_) => {
                self.word_proposals.pending = None;
                self.message = tl!("the word suggestion worker could not be read; the words are unchanged").into();
                return;
            }
        };
        let Some(result) = result else {
            if let Some(ctx) = &app.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            return;
        };
        let Some(pending) = self.word_proposals.pending.take() else { return };
        if pending.cancel.load(Ordering::Acquire) || !pending.context.current(self, &app.session) {
            return;
        }
        match result {
            Ok(candidates) if valid(&candidates) => {
                self.message =
                    if candidates.is_empty() { tl!("the word re-read found no suggestions; the words are unchanged").into() } else { String::new() };
                self.word_proposals.ready = Some(Ready { context: pending.context, candidates });
            }
            Ok(_) => self.message = tl!("the word re-read returned invalid suggestions; the words are unchanged").into(),
            Err(why) => self.message = why,
        }
    }

    pub(super) fn apply_word_candidate(&mut self, session: &Session, index: usize) -> bool {
        let Some(ready) = &self.word_proposals.ready else { return false };
        if !ready.context.current(self, session) {
            self.invalidate_word_candidates();
            self.message = tl!("the selected word changed; request suggestions again").into();
            return false;
        }
        let Some(candidate) = ready.candidates.get(index) else { return false };
        let (key, text) = (ready.context.key, candidate.text.clone());
        // `correct` is the existing local, undoable text-only review operation. Confidence,
        // geometry, source and page notes remain those of the original recognition.
        let changed = self.correct(key, &text);
        self.correction = self.text_of(key).unwrap_or_default().to_owned();
        self.invalidate_word_candidates();
        changed
    }
}

fn valid(candidates: &[WordCandidate]) -> bool {
    candidates.len() <= 2
        && candidates.iter().all(|candidate| {
            !candidate.text.is_empty()
                && candidate.text.len() <= 4096
                && !candidate.text.chars().any(char::is_whitespace)
                && !candidate.text.chars().any(char::is_control)
                && matches!(candidate.padding, 8 | 16)
                && candidate.source.len() <= 256
                && candidate.confidence.is_none_or(|confidence| confidence.is_finite() && (0.0..=100.0).contains(&confidence))
        })
}

pub(super) fn inspector(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, intents: &mut Intents) {
    ui.add_space(6.0);
    let review = &app.ocr_verify;
    let unavailable = review.proposal_context(&app.session).err();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!review.word_proposals.is_running() && unavailable.is_none(), egui::Button::new(tl!("Recognize text")).small())
            .on_disabled_hover_text(unavailable.as_deref().unwrap_or(tl!("A word re-read is still running")))
            .on_hover_text(tl!("Read only this word again. Suggestions do not change its text until you choose Apply."))
            .clicked()
        {
            intents.word_candidates = true;
        }
        if review.word_proposals.is_running() {
            ui.spinner();
            if ui.add(egui::Button::new(tl!("Cancel")).small()).clicked() {
                intents.cancel_word_candidates = true;
            }
        }
    });
    let Some(ready) = &review.word_proposals.ready else { return };
    if !ready.context.current(review, &app.session) || ready.candidates.is_empty() {
        return;
    }
    ui.label(egui::RichText::new(tl!("Suggestions")).small().color(t.text_muted));
    for (index, candidate) in ready.candidates.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            ui.label(&candidate.text);
            if ui.add(egui::Button::new(tl!("Apply")).small()).clicked() {
                intents.apply_word_candidate = Some(index);
            }
        });
        let confidence = candidate.confidence.map_or_else(|| "—".to_owned(), |c| format!("{c:.0}%"));
        ui.label(
            egui::RichText::new(crate::i18n::fmt(
                tl!("{label} · {source} · {confidence}"),
                &[("label", tl!("Confidence for this suggestion")), ("source", &candidate.source), ("confidence", &confidence)],
            ))
            .small()
            .color(t.text_muted),
        )
        .on_hover_text(crate::i18n::fmt(
            tl!("Confidence for this word crop only; the original word confidence is unchanged. Border: {padding} px."),
            &[("padding", &candidate.padding.to_string())],
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PdfCraftApp, OcrVerify) {
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_from_text("fixture", "Only synthetic review words").unwrap();
        let doc = app.session.open("word-proposals.pdf", None, bytes, None).unwrap();
        let settings = OcrSettings { language: "tr".into(), dpi: 240.0, ..Default::default() };
        let guard = app.session.ocr_job(doc, &[0], settings.clone()).unwrap().guard;
        let word = PlacedWord {
            text: "MAHKÜÛM.".into(),
            origin: [72.0, 700.0],
            across: [60.0, 0.0],
            up: [0.0, 12.0],
            confidence: Some(40.0),
            source: "tesseract".into(),
        };
        let mut review = OcrVerify::default();
        review.load_captured(
            doc,
            guard,
            vec![OcrPage { page: 0, words: vec![word], already_on_page: 0, skipped: None, notes: vec!["original note".into()] }],
            settings,
        );
        review.open = true;
        review.select((0, 0));
        (app, review)
    }

    fn candidate() -> WordCandidate {
        WordCandidate { text: "MAHKÛM.".into(), confidence: Some(93.0), padding: 8, source: "tesseract".into() }
    }

    fn pending(review: &mut OcrVerify, session: &Session, result: Option<Result<Vec<WordCandidate>, String>>) -> Arc<AtomicBool> {
        let context = review.proposal_context(session).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        review.word_proposals.pending = Some(Pending { context, cancel: cancel.clone(), result: Arc::new(Mutex::new(result)) });
        cancel
    }

    #[test]
    fn word_candidates_arrival_never_applies_and_explicit_apply_is_text_only_and_undoable() {
        let (mut app, mut review) = fixture();
        let before = review.word((0, 0)).unwrap().clone();
        let notes = review.page(0).unwrap().notes.clone();
        pending(&mut review, &app.session, Some(Ok(vec![candidate()])));
        review.poll_word_candidates(&mut app);
        assert_eq!(review.word((0, 0)), Some(&before), "a completed worker is only a proposal");
        assert!(review.review.history.is_empty());
        assert!(review.apply_word_candidate(&app.session, 0));
        let expected = PlacedWord { text: candidate().text, ..before.clone() };
        assert_eq!(review.word((0, 0)), Some(&expected));
        assert_eq!(review.page(0).unwrap().notes, notes);
        assert_eq!(review.review.history.len(), 1);
        assert!(review.undo());
        assert_eq!(review.word((0, 0)), Some(&before));
    }

    #[test]
    fn word_candidates_cancel_retains_worker_lease_and_discards_late_result() {
        let (mut app, mut review) = fixture();
        let cancel = pending(&mut review, &app.session, None);
        review.invalidate_word_candidates();
        assert!(cancel.load(Ordering::Acquire));
        assert!(review.word_proposals.is_running(), "cancel does not create room for a second native worker");
        *review.word_proposals.pending.as_ref().unwrap().result.lock().unwrap() = Some(Ok(vec![candidate()]));
        review.poll_word_candidates(&mut app);
        assert!(!review.word_proposals.is_running());
        assert!(review.word_proposals.ready.is_none());
        assert_eq!(review.text_of((0, 0)), Some("MAHKÜÛM."));
    }

    #[test]
    fn word_candidates_local_edit_then_undo_cannot_resurrect_inflight_proposal() {
        let (mut app, mut review) = fixture();
        let cancel = pending(&mut review, &app.session, Some(Ok(vec![candidate()])));
        assert!(review.correct((0, 0), "another word"));
        assert!(review.undo());
        review.poll_word_candidates(&mut app);
        assert!(cancel.load(Ordering::Acquire));
        assert!(review.word_proposals.ready.is_none());
        assert_eq!(review.text_of((0, 0)), Some("MAHKÜÛM."));
    }

    #[test]
    fn word_candidates_stale_document_and_changed_selection_refuse_apply() {
        let (mut app, mut review) = fixture();
        pending(&mut review, &app.session, Some(Ok(vec![candidate()])));
        review.poll_word_candidates(&mut app);
        review.selected = None;
        assert!(!review.apply_word_candidate(&app.session, 0));
        review.select((0, 0));
        pending(&mut review, &app.session, Some(Ok(vec![candidate()])));
        review.poll_word_candidates(&mut app);
        let doc = review.doc.unwrap();
        app.session.apply_ocr_checked(doc, &review.pages, review.guard.as_ref().unwrap()).unwrap();
        assert!(!review.apply_word_candidate(&app.session, 0));
        assert_eq!(review.text_of((0, 0)), Some("MAHKÜÛM."));
    }

    #[test]
    fn word_candidates_use_captured_settings_and_plain_load_never_guesses_them() {
        let (mut app, mut review) = fixture();
        app.ocr_draft.language = "en".into();
        app.ocr_draft.dpi = 600;
        let context = review.proposal_context(&app.session).unwrap();
        assert_eq!(context.settings.language, "tr");
        assert_eq!(context.settings.dpi, 240.0);
        let (doc, guard, pages) = (review.doc.unwrap(), review.guard.clone().unwrap(), review.pages.clone());
        review.load(doc, guard, pages);
        review.select((0, 0));
        assert!(review.proposal_context(&app.session).err().unwrap().contains("original recognition settings"));
    }

    #[test]
    fn word_candidates_error_empty_and_new_review_do_not_edit_words_or_drop_pending_lease() {
        let (mut app, mut review) = fixture();
        pending(&mut review, &app.session, Some(Err("synthetic bounded error".into())));
        review.poll_word_candidates(&mut app);
        assert_eq!(review.message, "synthetic bounded error");
        pending(&mut review, &app.session, Some(Ok(Vec::new())));
        review.poll_word_candidates(&mut app);
        assert_eq!(review.word_proposals.ready.as_ref().unwrap().candidates.len(), 0);
        let cancel = pending(&mut review, &app.session, Some(Ok(vec![candidate()])));
        review.finish();
        assert!(review.word_proposals.is_running());
        assert!(cancel.load(Ordering::Acquire));
        review.poll_word_candidates(&mut app);
        assert!(review.word_proposals.ready.is_none());
    }

    #[test]
    fn word_candidates_reused_index_and_changed_captured_profile_are_stale() {
        let (mut app, mut review) = fixture();
        pending(&mut review, &app.session, Some(Ok(vec![candidate()])));
        review.poll_word_candidates(&mut app);
        review.page_mut(0).unwrap().words[0].origin[0] += 1.0;
        assert!(!review.apply_word_candidate(&app.session, 0), "same index/text with another box is a different word");
        pending(&mut review, &app.session, Some(Ok(vec![candidate()])));
        review.captured_settings.as_mut().unwrap().language = "en".into();
        review.poll_word_candidates(&mut app);
        assert!(review.word_proposals.ready.is_none());
        assert_eq!(review.text_of((0, 0)), Some("MAHKÜÛM."));
    }

    #[test]
    fn word_candidates_reject_invalid_crop_confidence_and_do_not_modify_primary() {
        assert!(valid(&[candidate()]));
        assert!(!valid(&[WordCandidate { confidence: Some(f32::NAN), ..candidate() }]));
        assert!(!valid(&[WordCandidate { text: "two words".into(), ..candidate() }]));
        assert!(!valid(&[WordCandidate { padding: u32::MAX, ..candidate() }]));
    }
}

#[cfg(test)]
mod interaction_tests;
