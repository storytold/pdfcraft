//! The app's notices: every message and every running job's progress card goes through one `NoticeCenter`.
//!
//! `notify*` have no `Context` and are called from anywhere, so they push a [`PostedNotice`] onto the app's queue;
//! [`show`] drains it in the pass. A job's progress card is set where the job's count changes ([`set_progress`]) and
//! removed when the job ends ([`end_progress`]); both only queue a [`ProgressRequest`] that [`show`] applies with the
//! pass's `Context`, so none is lost before the first pass or from a headless caller. Nothing is mirrored every pass.

use craft::ui::notices::{NoticeEnd, PostedNotice};
use craft::ui::runtime::env::WindowKey;
use craft::ui::runtime::toast::{self, ToastId};

use crate::PdfCraftApp;

/// The jobs that hold a progress card, one card each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProgressKey {
    /// Optimize or Reduce File Size.
    Optimize,
    /// An export.
    Export,
    /// A long action.
    Action,
    /// A print job being spooled.
    Print,
    /// Scan clean-up.
    ScanClean,
    /// Text recognition, one run or a batch.
    Ocr,
}

/// A background job's progress, shown on its card until the job is done.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressNotice {
    /// What is happening ("Optimizing… image 3 of 40"): the card's message.
    pub label: String,
    /// From 0 to 1.
    pub fraction: f32,
    /// Show a Cancel action on the card.
    pub cancellable: bool,
}

/// A change to a job's card, applied by [`show`] in the order it was made.
#[derive(Clone, Debug, PartialEq)]
pub enum ProgressRequest {
    /// Set the card of the key: posts it the first time, moves it after.
    Set(ProgressKey, ProgressNotice),
    /// Remove the card of the key.
    End(ProgressKey),
}

/// The fixed card title of a job; the label is the message under it.
pub fn title(key: ProgressKey) -> String {
    match key {
        ProgressKey::Optimize => tl!("Optimizing").to_owned(),
        ProgressKey::Export => tl!("Exporting").to_owned(),
        ProgressKey::Action => tl!("Working").to_owned(),
        ProgressKey::Print => tl!("Printing").to_owned(),
        ProgressKey::ScanClean => tl!("Cleaning up scans").to_owned(),
        ProgressKey::Ocr => tl!("Recognizing text").to_owned(),
    }
}

/// Queues `notice` for the next pass, which posts it. A notice raised before the first pass or by a headless caller
/// waits here, so none is lost.
pub fn post(app: &mut PdfCraftApp, notice: PostedNotice) {
    app.posted_notices.push(notice);
}

/// Asks for the card of `key` to show `notice`: the first request posts it, later ones move the same card's fraction
/// and message. A card has no timer and carries Cancel when `notice.cancellable`.
pub fn set_progress(app: &mut PdfCraftApp, key: ProgressKey, notice: ProgressNotice) {
    app.progress_requests.push(ProgressRequest::Set(key, notice));
}

/// Asks for the card of `key` to go, when its job has ended.
pub fn end_progress(app: &mut PdfCraftApp, key: ProgressKey) {
    app.progress_requests.push(ProgressRequest::End(key));
}

/// Applies one progress request with the pass's `ctx`.
fn apply(app: &mut PdfCraftApp, ctx: &egui::Context, request: ProgressRequest) {
    match request {
        ProgressRequest::Set(key, notice) => {
            let fraction = if notice.fraction.is_finite() { notice.fraction.clamp(0.0, 1.0) } else { 0.0 };
            if let Some(id) = app.progress_cards.get(&key) {
                app.notices.set_progress(ctx, *id, fraction, Some(notice.label.into()));
                return;
            }
            let mut posted = PostedNotice::new(title(key)).message(notice.label).progress(fraction);
            if notice.cancellable {
                posted = posted.action(tl!("Cancel").to_owned());
            }
            if let Some(id) = app.notices.post(ctx, &posted, &[]) {
                app.progress_cards.insert(key, id);
            }
        }
        ProgressRequest::End(key) => {
            if let Some(id) = app.progress_cards.remove(&key) {
                app.notices.remove(ctx, id);
            }
        }
    }
}

/// How far the job of `key` has got, from its card; `None` when it has no card shown.
// PLAN: L-206 names this `Notices::progress(&self, key: &str)`; the app has no `Notices` type, so it is a function
// over the app's card map.
pub fn progress(app: &PdfCraftApp, ctx: &egui::Context, key: ProgressKey) -> Option<f32> {
    let id = app.progress_cards.get(&key)?;
    toast::cards(ctx, &WindowKey::main()).into_iter().find(|card| card.id == *id)?.toast.progress
}

/// The cards shown now, newest last.
// PLAN: L-206 names this `Notices::posted`; see `progress`.
pub fn posted(ctx: &egui::Context) -> Vec<toast::Card> {
    toast::cards(ctx, &WindowKey::main())
}

/// Posts the notices and applies the progress requests queued since the last pass, in the order they were made.
pub(crate) fn post_queued(app: &mut PdfCraftApp, ctx: &egui::Context) {
    // No PdfCraft notice sets a dismiss key, so nothing is ever silenced and the list is empty.
    // The same notice raised twice in one pass (a refusal reported by two layers) shows one card, as production's does.
    let mut posted: Vec<PostedNotice> = Vec::new();
    for notice in std::mem::take(&mut app.posted_notices) {
        if posted.contains(&notice) {
            continue;
        }
        app.notices.post(ctx, &notice, &[]);
        posted.push(notice);
    }
    for request in std::mem::take(&mut app.progress_requests) {
        apply(app, ctx, request);
    }
}

/// The text of every card shown in `ctx` after the app's queue is posted into it, oldest first: a card's title, then its
/// message.
#[cfg(test)]
pub(crate) fn texts(app: &mut PdfCraftApp, ctx: &egui::Context) -> Vec<String> {
    post_queued(app, ctx);
    posted(ctx)
        .iter()
        .map(|card| {
            let title = card.toast.title.as_ref().map(|t| t.as_str().to_owned());
            [title, Some(card.toast.message.as_str().to_owned())].into_iter().flatten().filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n")
        })
        .collect()
}

/// Posts the queued notices, collects how the notices ended and acts on them: a progress card's Cancel reaches its
/// job. Called once per pass. The ends are taken from the center's session state, so a second pass in the same frame
/// sees none of the first's.
pub fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    post_queued(app, &ctx);
    let ends = app.notices.clone().show(ui).ends;
    for end in ends {
        route(app, &end);
        // PLAN: assumed script_notice_ended(&mut self, &egui::Context, &NoticeEnd) (pdf-forms-w16)
        app.script_notice_ended(&ctx, &end);
    }
}

/// The notice an end is about.
fn ended_id(end: &NoticeEnd) -> Option<ToastId> {
    match end {
        NoticeEnd::Closed(id) | NoticeEnd::Expired(id) | NoticeEnd::Dropped(id) | NoticeEnd::Action(id) => Some(*id),
        NoticeEnd::Silenced { id, .. } => Some(*id),
        _ => None,
    }
}

/// Forgets the card that ended; its action is the job's Cancel.
fn route(app: &mut PdfCraftApp, end: &NoticeEnd) {
    let Some(id) = ended_id(end) else { return };
    let Some(key) = app.progress_cards.iter().find(|(_, card)| **card == id).map(|(key, _)| *key) else { return };
    app.progress_cards.remove(&key);
    if !matches!(end, NoticeEnd::Action(_)) {
        return;
    }
    match key {
        ProgressKey::Optimize => app.cancel_optimize(),
        ProgressKey::ScanClean => app.cancel_scan_clean(),
        ProgressKey::Ocr if app.ocr_batch.is_some() => app.cancel_ocr_batch(),
        ProgressKey::Ocr => app.cancel_ocr(),
        // These jobs offer no Cancel.
        ProgressKey::Export | ProgressKey::Action | ProgressKey::Print => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JobCount;
    use crate::workflows::optimize_ui::{OptimizeKind, OptimizeRun};
    use crate::workflows::scan_clean_ui::ScanCleanRun;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use pdfcraft_engine::DocId;
    use pdfcraft_engine::optimizer::OptimizeStage;
    use std::sync::{Arc, Mutex};

    fn card_of(h: &Harness<'static, PdfCraftApp>, key: ProgressKey) -> toast::Card {
        let id = h.state().progress_cards.get(&key).copied().expect("the job has a card");
        posted(&h.ctx).into_iter().find(|card| card.id == id).expect("the card is shown")
    }

    /// A second `set_progress` for a key moves the same card; a Cancel click reaches only its own job; ending a job
    /// removes its card.
    #[test]
    fn a_progress_card_is_one_per_key_and_its_cancel_reaches_its_job() {
        let mut h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(|cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let mut app = PdfCraftApp::for_unit_tests();
                app.set_option(&egui::Context::default(), "language", "en").unwrap();
                app
            }
        });
        h.run_steps(2);

        let optimize = ProgressNotice { label: "Optimizing… reading the pages".into(), fraction: 0.25, cancellable: true };
        set_progress(h.state_mut(), ProgressKey::Optimize, optimize.clone());
        set_progress(
            h.state_mut(),
            ProgressKey::ScanClean,
            ProgressNotice { label: "Cleaning image 1 of 4".into(), fraction: 0.1, cancellable: false },
        );
        h.run_steps(2);
        let first = card_of(&h, ProgressKey::Optimize).id;

        // Repeated calls move the one card.
        set_progress(
            h.state_mut(),
            ProgressKey::Optimize,
            ProgressNotice { label: "Optimizing… image 3 of 40".into(), fraction: 0.75, cancellable: true },
        );
        h.run_steps(2);
        let cards = posted(&h.ctx);
        assert_eq!(cards.len(), 2, "one card per key: {cards:?}");
        let moved = card_of(&h, ProgressKey::Optimize);
        assert_eq!(moved.id, first);
        let before = 0.25;
        assert!(progress(h.state(), &h.ctx, ProgressKey::Optimize).is_some_and(|now| now > before), "the same card moved on");
        assert_eq!(moved.toast.message.as_str(), "Optimizing… image 3 of 40");
        assert_eq!(moved.toast.title.as_ref().map(|t| t.as_str().to_owned()), Some(title(ProgressKey::Optimize)));
        assert!(progress(h.state(), &h.ctx, ProgressKey::ScanClean).is_some());

        // Both jobs are in flight; Cancel is on the Optimize card only.
        let optimize_progress = Arc::new(JobCount::default());
        let scan_progress = Arc::new(JobCount::default());
        h.state_mut().optimize_run = Some(OptimizeRun {
            doc: DocId(1),
            kind: OptimizeKind::Reduce,
            destination: None,
            progress: optimize_progress.clone(),
            stage: Arc::new(Mutex::new(OptimizeStage::Discarding)),
        });
        h.state_mut().scan_clean_run = Some(ScanCleanRun { doc: DocId(1), progress: scan_progress.clone() });
        h.get_by_label("Cancel").click();
        h.run_steps(3);
        assert!(optimize_progress.cancelled(), "Cancel reaches the Optimize job");
        assert!(!scan_progress.cancelled(), "the ScanClean job is left running");
        assert!(h.state().progress_cards.contains_key(&ProgressKey::ScanClean));
        assert!(posted(&h.ctx).iter().any(|card| card.id == card_of(&h, ProgressKey::ScanClean).id));

        // Ending a job removes its card.
        h.state_mut().optimize_run = None;
        h.state_mut().scan_clean_run = None;
        end_progress(h.state_mut(), ProgressKey::Optimize);
        end_progress(h.state_mut(), ProgressKey::ScanClean);
        h.run_steps(2);
        assert!(posted(&h.ctx).is_empty());
        assert!(h.state().progress_cards.is_empty());
    }
}
