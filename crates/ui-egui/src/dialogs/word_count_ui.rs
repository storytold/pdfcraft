//! Read-only statistics for a document snapshot. At most one worker per dialog, with cancellation
//! between pages and no stale results after edits, page changes or changed exclusions.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};

use pdfcraft_engine::{
    DocId,
    export::ExportSource,
    word_count::{self, Options, Report},
};

use crate::{PdfCraftApp, widgets};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    doc: DocId,
    generation: u64,
    page: Option<usize>,
    options: Options,
}

struct Job {
    key: Key,
    receiver: mpsc::Receiver<Result<Report, String>>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(crate) struct Draft {
    options: Options,
    current_page: bool,
    key: Option<Key>,
    job: Option<Job>,
    result: Option<Result<Report, String>>,
}

impl Draft {
    pub(crate) fn cancel(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.key = None;
        self.result = None;
    }

    fn update(&mut self, key: Key, source: impl FnOnce() -> ExportSource, ctx: &egui::Context, inline: bool) {
        if self.key != Some(key) {
            self.cancel();
            self.key = Some(key);
        }
        if let Some(job) = &self.job {
            let received = match job.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(tl!("Word counting stopped unexpectedly.").into())),
            };
            if let Some(result) = received {
                if job.key == key && !job.cancel.load(Ordering::Relaxed) {
                    self.result = Some(result);
                }
                self.job = None;
            }
        }
        if self.job.is_some() || self.result.is_some() {
            return;
        }
        let source = source();
        let pages: Vec<_> = key.page.map_or_else(|| (0..source.pages).collect(), |page| vec![page]);
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicUsize::new(0));
        self.job = Some(Job { key, receiver, cancel: cancel.clone(), done: done.clone() });
        let ctx = ctx.clone();
        let work = move || {
            let result = word_count::count(source, &pages, key.options, |n| {
                done.store(n, Ordering::Relaxed);
                ctx.request_repaint();
                !cancel.load(Ordering::Relaxed)
            });
            let _ = sender.send(result);
            ctx.request_repaint();
        };
        #[cfg(not(target_arch = "wasm32"))]
        if !inline {
            if let Err(error) = std::thread::Builder::new().name("word-count".into()).spawn(work) {
                self.job = None;
                self.result = Some(Err(error.to_string()));
            }
            return;
        }
        let _ = inline;
        work();
    }
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> bool {
    ui.heading(tl!("Word count"));
    let Some((view, id)) = app.active_ids() else { return true };
    let Some(doc) = app.session.get(id) else { return true };
    if let Err(error) = doc.check_copying() {
        app.word_count_draft.cancel();
        ui.label(error.to_string());
        return false;
    }
    let draft = &mut app.word_count_draft;
    ui.add(widgets::checkbox(&mut draft.current_page, tl!("Current page only")));
    ui.separator();
    ui.add(widgets::checkbox(&mut draft.options.exclude_tables, tl!("Exclude tagged tables")));
    ui.add(widgets::checkbox(&mut draft.options.exclude_code, tl!("Exclude tagged code blocks")));
    ui.add(widgets::checkbox(&mut draft.options.exclude_images, tl!("Exclude image/OCR text")));
    ui.label(tl!("Exclusions use PDF tags. Untagged tables and code remain included. Image exclusion also removes invisible text."));
    let key = Key { doc: id, generation: doc.edit_generation(), page: draft.current_page.then_some(app.views[view].current), options: draft.options };
    draft.update(key, || doc.export_source(), ui.ctx(), app.run_inline);
    ui.separator();
    match &draft.result {
        Some(Ok(report)) => {
            egui::Grid::new("word-count-totals").show(ui, |ui| {
                for (label, value) in [
                    (tl!("Words"), report.total.words),
                    (tl!("Characters"), report.total.characters),
                    (tl!("Characters without spaces"), report.total.characters_without_spaces),
                    (tl!("Pages counted"), report.pages.len()),
                ] {
                    ui.label(label);
                    ui.strong(value.to_string());
                    ui.end_row();
                }
            });
            if report.pages_without_text > 0 {
                ui.label(tl!("Some pages contain no countable text. Scanned images need OCR first."));
            }
            if report.unclassified_structure {
                ui.label(tl!("Some content could not be classified safely and was kept in the count."));
            }
        }
        Some(Err(error)) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        None => {
            ui.spinner();
            let done = draft.job.as_ref().map_or(0, |job| job.done.load(Ordering::Relaxed));
            ui.label(crate::i18n::fmt(tl!("Counting words… {n} pages read"), &[("n", &done.to_string())]));
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Dialog;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn word_count_dialog_counts_changes_pages_and_closes() {
        for inline in [true, false] {
            let bytes = pdfcraft_engine::Session::new().create_from_text("Words", "Hello world\u{c}Another three words").unwrap();
            let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(|_| {
                let mut app = PdfCraftApp::new();
                app.run_inline = inline;
                app.set_option("language", "en").unwrap();
                app.open_bytes("words.pdf", None, bytes.as_ref().clone()).unwrap();
                app.run_command("file.word_count");
                app
            });
            let wait = |h: &mut Harness<'_, PdfCraftApp>| {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while h.state().word_count_draft.result.is_none() {
                    assert!(std::time::Instant::now() < deadline, "word-count worker timed out");
                    h.run_steps(1);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                h.run_steps(2);
            };
            wait(&mut h);
            assert_eq!(h.state().dialog, Some(Dialog::WordCount));
            assert_eq!(h.state().word_count_draft.result.as_ref().unwrap().as_ref().unwrap().total.words, 5);
            h.get_by_label("Current page only").click();
            h.run_steps(1);
            wait(&mut h);
            assert_eq!(h.state().word_count_draft.result.as_ref().unwrap().as_ref().unwrap().total.words, 2);
            h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![0] });
            h.run_steps(1);
            wait(&mut h);
            assert_eq!(h.state().word_count_draft.result.as_ref().unwrap().as_ref().unwrap().total.words, 3);
            h.get_by_label("Close").click();
            h.run_steps(2);
            assert!(h.state().dialog.is_none());
            assert!(h.state().word_count_draft.result.is_none());
        }
    }

    #[test]
    fn word_count_cancelled_worker_cannot_publish_an_old_result() {
        let key = Key { doc: DocId(1), generation: 0, page: None, options: Options::default() };
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut draft = Draft {
            key: Some(key),
            job: Some(Job { key, receiver: rx, cancel: cancel.clone(), done: Arc::new(AtomicUsize::new(0)) }),
            ..Draft::default()
        };
        draft.cancel();
        assert!(cancel.load(Ordering::Relaxed));
        let bytes = pdfcraft_engine::Session::new().create_blank(200.0, 300.0, 1).unwrap();
        let source = || ExportSource { bytes: bytes.clone(), pages: 1, sizes: vec![(200.0, 300.0)], config: Default::default() };
        draft.update(key, source, &egui::Context::default(), true);
        assert!(draft.result.is_none()); // still one worker, no competing replacement
        tx.send(Ok(Report { total: word_count::Counts { words: 999, ..Default::default() }, ..Default::default() })).unwrap();
        draft.update(key, source, &egui::Context::default(), true);
        draft.update(key, source, &egui::Context::default(), true);
        assert_eq!(draft.result.unwrap().unwrap().total.words, 0);
    }
}
