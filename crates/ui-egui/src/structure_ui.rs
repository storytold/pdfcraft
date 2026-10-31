//! Standards ▸ Validate structure: the document checked against the Arlington PDF Model, and the
//! list of what departs from it. Choosing a finding on a page turns to that page and flashes the
//! annotation it is about; the report stays, so the command shows it again until the document
//! changes.
//!
//! A large document takes a second or more to check, so the check runs on a worker thread
//! (inline before the first frame, in tests and on the web), on a snapshot of the object graph,
//! and the dialog counts the objects checked until the report arrives. Cancel stops it; Close
//! leaves it running, and opening the dialog again shows how far it is, or its report.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak, mpsc};

use egui::{Align, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::structure::{Finding, MODEL_COMMIT, Options, Report, Severity};

use crate::theme::{self, Tokens};
use crate::{Dialog, PdfCraftApp, icons, widgets};

/// Rows the list lays out at once are this tall: the finding, then where it is.
const ROW_HEIGHT: f32 = 38.0;

/// How often the dialog looks at a running check, for the count it shows and the report.
const POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// The last check, the one running, and which severities the list shows.
#[derive(Debug)]
pub struct StructureState {
    /// What the last check came to, with the document and working file it was of.
    last: Option<(Key, Checked)>,
    job: Option<Job>,
    /// Errors, warnings, information: shown or not. Information (keys the model doesn't
    /// define) starts hidden; there can be many and none is a problem.
    pub show: [bool; 3],
}

impl Default for StructureState {
    fn default() -> Self {
        StructureState { last: None, job: None, show: [true, true, false] }
    }
}

/// The document a check is of, as the working file it was then (each edit makes a new one).
#[derive(Clone, Debug)]
struct Key {
    doc: DocId,
    bytes: Weak<Vec<u8>>,
}

impl Key {
    fn of(doc: &pdfcraft_engine::Document) -> Key {
        Key { doc: doc.id, bytes: Arc::downgrade(&doc.bytes) }
    }

    fn is(&self, doc: &pdfcraft_engine::Document) -> bool {
        self.same(&Key::of(doc))
    }

    fn same(&self, other: &Key) -> bool {
        self.doc == other.doc && Weak::ptr_eq(&self.bytes, &other.bytes)
    }
}

/// What a check came to.
#[derive(Debug)]
enum Checked {
    Report(Report),
    /// The document has no object graph to check (it couldn't be read for editing).
    Unreadable,
    /// Cancel stopped it before there was a report.
    Cancelled,
    /// The worker failed (the validator never should): how, in English, for a bug report.
    Failed(String),
}

/// A check running on a worker thread. Dropping it asks the worker to stop.
#[derive(Debug)]
struct Job {
    key: Key,
    receiver: mpsc::Receiver<Result<Option<Report>, String>>,
    cancel: Arc<AtomicBool>,
    /// Objects checked so far.
    checked: Arc<AtomicUsize>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What a [`Job`] runs, given the callback that takes the progress and says whether to go on.
type Work = Box<dyn FnOnce(&mut dyn FnMut(usize) -> bool) -> Result<Option<Report>, String> + Send>;

impl StructureState {
    /// Run `work` for the document `key` on a worker thread, or here when `inline`, in place of
    /// any check still running (which is told to stop). `ctx` is asked to repaint when it's done.
    fn start(&mut self, key: Key, work: Work, ctx: Option<egui::Context>, inline: bool) {
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let checked = Arc::new(AtomicUsize::new(0));
        let (stop, count) = (cancel.clone(), checked.clone());
        let run = move || {
            let result = work(&mut |n: usize| {
                count.store(n, Ordering::Relaxed);
                !stop.load(Ordering::Relaxed)
            });
            // Nobody is waiting once the dialog moved on to a newer check or was cancelled.
            let _ = sender.send(result);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        };
        self.job = Some(Job { key: key.clone(), receiver, cancel, checked });
        #[cfg(not(target_arch = "wasm32"))]
        if !inline {
            if let Err(e) = std::thread::Builder::new().name("pdfcraft-structure".into()).spawn(run) {
                // No worker: say so instead of waiting for a report that never comes.
                self.job = None;
                self.last = Some((key, Checked::Failed(e.to_string())));
            }
            return;
        }
        let _ = inline;
        run();
    }

    /// Take the report of a check that is done.
    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        let checked = match job.receiver.try_recv() {
            Ok(Ok(Some(report))) => Checked::Report(report),
            Ok(Ok(None)) => Checked::Cancelled,
            Ok(Err(e)) => Checked::Failed(e),
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Checked::Failed("the worker stopped without a report".into()),
        };
        if let Some(job) = self.job.take() {
            self.last = Some((job.key.clone(), checked));
        }
    }
}

fn slot(s: Severity) -> usize {
    match s {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    }
}

fn icon(t: &Tokens, s: Severity) -> (&'static str, egui::Color32) {
    match s {
        Severity::Error => ("circle-x", egui::Color32::from_rgb(0xD7, 0x37, 0x3F)),
        Severity::Warning => ("triangle-alert", egui::Color32::from_rgb(0xD9, 0x8E, 0x04)),
        Severity::Info => ("info", t.text_faint),
    }
}

impl PdfCraftApp {
    /// Check the active document (the dialog does when it has no report of the document as it
    /// is now, and so does Check again): on a worker thread once the window is up, so a large
    /// document doesn't freeze it; here before the first frame, in tests and on the web.
    pub fn validate_structure(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let key = Key::of(doc);
        let Some(job) = doc.structure_job(&Options::default()) else {
            self.structure.job = None;
            self.structure.last = Some((key, Checked::Unreadable));
            self.notify_tr("This document can't be checked: it couldn't be read for editing");
            return;
        };
        let inline = self.ctx.is_none() || self.run_inline;
        self.structure.start(key, Box::new(move |progress: &mut dyn FnMut(usize) -> bool| job.run(progress)), self.ctx.clone(), inline);
        if inline {
            // Done already: the report is there for whoever asked.
            self.structure.poll();
        }
    }

    /// The report of the active document as it is now, if there is one.
    pub fn structure_report(&self) -> Option<&Report> {
        match self.structure_checked()? {
            Checked::Report(report) => Some(report),
            _ => None,
        }
    }

    /// While the active document as it is now is being checked: how many objects are done.
    pub fn structure_checking(&self) -> Option<usize> {
        let doc = self.active_ids().and_then(|(_, id)| self.session.get(id))?;
        self.structure.job.as_ref().filter(|job| job.key.is(doc)).map(|job| job.checked.load(Ordering::Relaxed))
    }

    /// Stop the running check. A report it was to replace (Check again) stays.
    pub fn cancel_structure(&mut self) {
        let Some(job) = self.structure.job.take() else { return };
        if !self.structure.last.as_ref().is_some_and(|(key, _)| key.same(&job.key)) {
            self.structure.last = Some((job.key.clone(), Checked::Cancelled));
        }
    }

    /// What the last check of the active document as it is now came to.
    fn structure_checked(&self) -> Option<&Checked> {
        let doc = self.active_ids().and_then(|(_, id)| self.session.get(id))?;
        self.structure.last.as_ref().filter(|(key, _)| key.is(doc)).map(|(_, checked)| checked)
    }

    /// Take the report of a check that is done, and check the active document unless there is
    /// a report of it as it is now or a check of it running. So the dialog has something to
    /// show however it was opened (the command, the control channel's `set dialog structure`),
    /// and after an edit.
    pub(crate) fn update_structure(&mut self) {
        self.structure.poll();
        if self.structure_checking().is_none() && self.structure_checked().is_none() {
            self.validate_structure();
        }
    }

    /// Standards ▸ Validate structure: the dialog, checking first unless the last check is of
    /// the document as it is now.
    pub(crate) fn open_structure(&mut self) {
        self.update_structure();
        self.dialog = Some(Dialog::Structure);
    }

    /// Turn to a finding's page and flash the annotation it is about.
    pub(crate) fn structure_show(&mut self, page: usize, rect: Option<[f64; 4]>) {
        let Some((index, _)) = self.active_ids() else { return };
        let Some(view) = self.views.get_mut(index) else { return };
        view.go_to_page(page);
        if let Some(rect) = rect {
            view.flash = Some((page, rect.map(|v| v as f32), 0.0));
        }
    }
}

/// Where a finding is, as its second line says it.
fn place(f: &Finding) -> String {
    let mut parts = vec![f.path.clone()];
    if let Some(o) = f.object {
        parts.push(o.to_string());
    }
    if let Some(p) = f.page {
        parts.push(crate::i18n::fmt(tl!("Page {label}"), &[("label", &(p + 1).to_string())]));
    }
    parts.join(" · ")
}

/// Returns `true` to close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    app.update_structure();
    ui.set_width(640.0);
    ui.label(egui::RichText::new(tl!("Validate structure")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let mut close = false;
    let mut again = false;
    let mut cancel = false;
    let mut show: Option<(usize, Option<[f64; 4]>)> = None;
    let shown = app.structure.show;
    let mut toggles = shown;
    let checking = app.structure_checking();
    match (checking, app.structure_checked()) {
        (Some(done), _) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(crate::i18n::fmt(tl!("Checking the structure… {n} objects so far"), &[("n", &done.to_string())]));
            });
            // The count moves on, and the report arrives, without any input to repaint for.
            ui.ctx().request_repaint_after(POLL);
        }
        (None, None | Some(Checked::Unreadable)) => {
            ui.label(egui::RichText::new(tl!("This document can't be checked: it couldn't be read for editing")).color(t.text_muted));
        }
        (None, Some(Checked::Cancelled)) => {
            ui.label(egui::RichText::new(tl!("The check was cancelled.")).color(t.text_muted));
        }
        (None, Some(Checked::Failed(why))) => {
            ui.label(tl!("The check stopped unexpectedly."));
            ui.label(egui::RichText::new(why).small().color(t.text_faint));
        }
        (None, Some(Checked::Report(report))) => {
            ui.label(crate::i18n::fmt(
                tl!("Checked {objects} objects against the Arlington PDF Model for PDF {version}."),
                &[("objects", &report.objects.to_string()), ("version", &report.version.to_string())],
            ));
            if report.is_valid() {
                // Only a check that evaluated every rule, over the whole document, can say the
                // structure follows the model; the notes below say what wasn't evaluated.
                let complete = report.not_checked.is_empty() && !report.stopped_early;
                let said = if complete {
                    tl!("No errors: the structure follows the Arlington PDF Model.")
                } else {
                    tl!("No errors in the rules that were evaluated.")
                };
                ui.label(egui::RichText::new(said).color(t.text_muted));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (i, (label, count)) in
                    [(tl!("Errors"), report.errors), (tl!("Warnings"), report.warnings), (tl!("Information"), report.infos)].into_iter().enumerate()
                {
                    if let Some(on) = toggles.get_mut(i) {
                        ui.add(widgets::checkbox(on, format!("{label} ({count})")));
                    }
                }
            });
            ui.add_space(4.0);
            let rows: Vec<&Finding> = report.findings.iter().filter(|f| shown.get(slot(f.severity)).copied().unwrap_or(false)).collect();
            egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(160.0);
                egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
                    for f in rows.get(range).unwrap_or_default() {
                        let (name, colour) = icon(t, f.severity);
                        // The validator hands over its sentence and the document's own words apart,
                        // so the sentence reads in this language with the names as they are.
                        let said = f.said(&crate::i18n::words);
                        let row = ui
                            .horizontal(|ui| {
                                ui.set_height(ROW_HEIGHT);
                                ui.add(icons::image(name, 15.0, colour));
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    let line = ui.add(
                                        egui::Label::new(egui::RichText::new(&said).font(theme::regular(12.5)))
                                            .truncate()
                                            .sense(egui::Sense::click()),
                                    );
                                    ui.add(egui::Label::new(egui::RichText::new(place(f)).small().color(t.text_muted)).truncate());
                                    line
                                })
                                .inner
                            })
                            .inner;
                        let row = row.on_hover_text(&said);
                        if let Some(page) = f.page
                            && row.on_hover_text(tl!("Go to the page")).clicked()
                        {
                            show = Some((page, f.rect));
                        }
                    }
                });
            });
            if !report.not_checked.is_empty() {
                let list: Vec<String> = report.not_checked.iter().take(8).map(|(what, n)| format!("{what} ({n})")).collect();
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(tl!("Not evaluated: {list}"), &[("list", &list.join(", "))])).small().color(t.text_muted),
                );
            }
            if report.truncated {
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        tl!("Only the first {n} of each kind are listed."),
                        &[("n", &Options::default().max_findings.to_string())],
                    ))
                    .small()
                    .color(t.text_muted),
                );
            }
            if report.stopped_early {
                ui.label(
                    egui::RichText::new(tl!("The document has more objects than one check covers; the rest was not checked."))
                        .small()
                        .color(t.text_muted),
                );
            }
            let commit = MODEL_COMMIT.get(..8).unwrap_or(MODEL_COMMIT);
            ui.label(egui::RichText::new(format!("Arlington PDF Model (PDF Association, Apache-2.0) {commit}")).small().color(t.text_faint));
        }
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Close"), true).clicked() {
                close = true;
            }
            if checking.is_some() {
                cancel = widgets::pill_button(ui, tl!("Cancel"), false).clicked();
            } else if widgets::pill_button(ui, tl!("Check again"), false).clicked() {
                again = true;
            }
        });
    });
    app.structure.show = toggles;
    if cancel {
        app.cancel_structure();
    }
    if again {
        app.validate_structure();
    }
    if let Some((page, rect)) = show {
        app.structure_show(page, rect);
        close = true;
    }
    close
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use pdfcraft_engine::structure::{Options, Report};

    use super::{Key, Work};
    use crate::i18n::{Lang, has, tr};
    use crate::{Dialog, PdfCraftApp};

    fn harness() -> Harness<'static, PdfCraftApp> {
        let bytes = pdfcraft_engine::Session::new().create_from_text("Structure", "Hello").unwrap();
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(move |_| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("structure.pdf", None, bytes.as_ref().clone()).unwrap();
            app
        });
        h.run_steps(2);
        h
    }

    /// A held check of the active document, started as the dialog starts one, with the dialog
    /// open: it says it has got through 42 objects, then waits for `release` (or gives up after a
    /// few seconds, so a check run on the UI thread fails the test instead of hanging it), and
    /// then finishes with the real report unless it was told to stop, which `stopped` records.
    struct Held {
        release: mpsc::Sender<()>,
        stopped: Arc<AtomicBool>,
        report: Report,
    }

    fn hold(h: &mut Harness<'_, PdfCraftApp>) -> Held {
        let (release, wait) = mpsc::channel::<()>();
        let stopped = Arc::new(AtomicBool::new(false));
        let app = h.state_mut();
        let doc = app.active_ids().and_then(|(_, id)| app.session.get(id)).unwrap();
        let report = doc.validate_structure(&Options::default()).unwrap();
        let (key, done, told) = (Key::of(doc), report.clone(), stopped.clone());
        let work: Work = Box::new(move |progress: &mut dyn FnMut(usize) -> bool| {
            progress(42);
            let _ = wait.recv_timeout(Duration::from_secs(10));
            if progress(43) {
                Ok(Some(done))
            } else {
                told.store(true, Ordering::Relaxed);
                Ok(None)
            }
        });
        let ctx = app.ctx.clone();
        assert!(ctx.is_some() && !app.run_inline, "the window is up");
        app.structure.start(key, work, ctx, false);
        app.dialog = Some(Dialog::Structure);
        Held { release, stopped, report }
    }

    fn until(h: &mut Harness<'_, PdfCraftApp>, what: &str, done: impl Fn(&PdfCraftApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(h.state()) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(2));
        }
        h.run_steps(2);
    }

    /// The window goes on painting and answering while a check runs: the dialog counts the
    /// objects checked, Close leaves the check running, and its report arrives by itself.
    #[test]
    fn the_window_goes_on_while_the_check_runs() {
        let mut h = harness();
        let held = hold(&mut h);
        // The check runs on its own thread: it is still under way once `hold` returns, where a
        // check on the UI thread would have finished (or given up) first.
        assert!(h.state().structure_checking().is_some(), "the check runs while frames go on");
        // The worker counts its first objects once its thread runs, which on a slow runner takes
        // more than a frame or two: wait for the count, not for a number of frames.
        until(&mut h, "the first count", |app| app.structure_checking() == Some(42));
        h.get_by_label("Checking the structure… 42 objects so far");
        assert_eq!(h.state().structure_checking(), Some(42));
        assert!(h.query_by_label("Check again").is_none(), "Cancel, not Check again, while it runs");

        h.get_by_label("Close").click();
        h.run_steps(2);
        assert_eq!(h.state().dialog, None);
        assert_eq!(h.state().structure_checking(), Some(42), "closing doesn't stop it");
        assert!(h.state_mut().execute("standards.structure"));
        h.run_steps(2);
        h.get_by_label("Checking the structure… 42 objects so far");

        held.release.send(()).unwrap();
        until(&mut h, "the report", |app| app.structure_report().is_some());
        assert_eq!(h.state().structure_report(), Some(&held.report));
        assert!(!held.stopped.load(Ordering::Relaxed));
        let report = &held.report;
        h.get_by_label(&format!("Checked {} objects against the Arlington PDF Model for PDF {}.", report.objects, report.version));
    }

    /// Cancel stops the worker; the dialog says so until Check again. A check that was to
    /// replace a report (Check again) leaves that report when it's cancelled.
    #[test]
    fn cancel_stops_the_check_and_keeps_the_last_report() {
        let mut h = harness();
        let held = hold(&mut h);
        h.run_steps(2);
        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert_eq!(h.state().structure_checking(), None);
        h.get_by_label("The check was cancelled.");
        drop(held.release);
        until(&mut h, "the worker to stop", |_| held.stopped.load(Ordering::Relaxed));
        h.get_by_label("The check was cancelled.");

        h.get_by_label("Check again").click();
        until(&mut h, "the report", |app| app.structure_report().is_some());
        let again = hold(&mut h);
        h.run_steps(2);
        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert_eq!(h.state().structure_report(), Some(&again.report), "the report stays");
        h.get_by_label("Check again");
    }

    /// An edit while a check runs makes it out of date: it is stopped and the document as it is
    /// now is checked instead.
    #[test]
    fn an_edit_while_checking_starts_over() {
        let mut h = harness();
        let held = hold(&mut h);
        h.run_steps(2);
        assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Draft".into() }));
        h.run_steps(1);
        drop(held.release);
        // The held check, let go, finds it was told to stop; the new one reports.
        until(&mut h, "the new report", |app| app.structure_report().is_some() && held.stopped.load(Ordering::Relaxed));
        let app = h.state();
        let edited = app.active_ids().and_then(|(_, id)| app.session.get(id)).and_then(|d| d.validate_structure(&Options::default()));
        assert_eq!(app.structure_report(), edited.as_ref(), "the report is of the document as edited");
    }

    /// Spanish says every sentence the validator can, and keeps each of its holes, so the
    /// document's own names still have somewhere to go.
    #[test]
    fn spanish_covers_what_validate_structure_says() {
        let es = Lang::from_code("es").expect("es registered");
        assert!(!pdfcraft_engine::structure::WORDINGS.is_empty());
        for wording in pdfcraft_engine::structure::WORDINGS {
            assert!(has(es, wording), "missing: {wording}");
            let said = tr(es, wording);
            for hole in wording.split('{').skip(1).filter_map(|rest| rest.split_once('}')).map(|(name, _)| name) {
                assert!(said.contains(&format!("{{{hole}}}")), "the Spanish of {wording:?} loses {{{hole}}}: {said:?}");
            }
        }
    }
}
