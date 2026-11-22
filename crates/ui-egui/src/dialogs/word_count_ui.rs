//! Read-only statistics for a document snapshot. The count runs on the host's job queue; changing the page choice,
//! the exclusions or the document cancels the running count, and closing the dialog drops the queue.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use craft::host::Wake;
use craft::host::queue::{Cancel, JobQueue, Priority, QueueOptions};
use craft::theme::variant::Tone;
use craft::ui::choice::Checkbox;
use craft::ui::contract::{Bind, Props as _, Show as _, ShowContainer as _};
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, StackDialog, dialog_id};
use craft::ui::display::{Shade, Spinner, Text};
use craft::ui::form::Field;
use craft::ui::layout_components::Row;
use craft::ui::notices::{Notice, NoticeLayout};
use craft::ui::patterns::form_dialogs::FormDialog;
use craft::ui::sections::{DialogRole, FooterButton};
use craft::ui::style::TypeRole;
use egui::Ui;
use pdfcraft_engine::{
    DocId,
    export::ExportSource,
    word_count::{self, Options, Report},
};

use crate::PdfCraftApp;

/// What the dialog counted for: a change in any part starts a new count.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    doc: DocId,
    generation: u64,
    page: Option<usize>,
    options: Options,
}

/// The choices the dialog remembers across openings.
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) struct WordCountOptions {
    pub options: Options,
    pub current_page: bool,
}

/// Word count, owned by the dialog host.
pub(crate) struct WordCountDialog {
    id: DialogId,
    options: WordCountOptions,
    /// What the running or finished count is for.
    key: Option<Key>,
    // PLAN: `JobQueue` keys must be `Hash`, which `Options` is not known to be: jobs are keyed by `serial`.
    queue: JobQueue<u64, Result<Report, String>>,
    /// The serial of the count for `key`; a result of any other serial is stale.
    serial: u64,
    wake: Wake,
    /// Pages the running count has read.
    done: Arc<AtomicUsize>,
    shown: Option<Result<Report, String>>,
}

impl WordCountDialog {
    fn new(options: WordCountOptions, queue: JobQueue<u64, Result<Report, String>>, wake: Wake) -> Self {
        WordCountDialog { id: dialog_id("word-count"), options, key: None, queue, serial: 0, wake, done: Arc::new(AtomicUsize::new(0)), shown: None }
    }

    /// Whether the count for the current key has finished (with a report or an error).
    pub(crate) fn finished(&self) -> bool {
        self.shown.is_some()
    }

    /// Shows `message` in place of a count and cancels any running one.
    fn refuse(&mut self, message: String) {
        self.queue.reprioritize(|_, _| None);
        self.key = None;
        self.shown = Some(Err(message));
    }

    /// Starts a count when `key` is not the one counted, cancelling the old one, and takes the result that arrived
    /// for the current count.
    fn update(&mut self, key: Key, source: impl FnOnce() -> ExportSource) {
        if self.key != Some(key) {
            self.queue.reprioritize(|_, _| None);
            self.key = Some(key);
            self.shown = None;
            self.serial += 1;
            let source = source();
            let pages: Vec<usize> = key.page.map_or_else(|| (0..source.pages).collect(), |page| vec![page]);
            let done = Arc::new(AtomicUsize::new(0));
            self.done = done.clone();
            let wake = self.wake.clone();
            self.queue.submit(self.serial, Priority::VISIBLE, move |cancel: &Cancel| {
                word_count::count(source, &pages, key.options, |n| {
                    done.store(n, Ordering::Relaxed);
                    wake.wake();
                    !cancel.is_cancelled()
                })
            });
        }
        while let Some(finished) = self.queue.try_recv() {
            if finished.key == self.serial {
                self.shown = Some(finished.result);
            }
        }
    }
}

impl StackDialog for WordCountDialog {
    type Result = ();

    fn id(&self) -> DialogId {
        self.id
    }

    fn show(&mut self, ui: &mut Ui, _: &DialogStack) -> Option<DialogResult<()>> {
        let WordCountDialog { options, shown, done, key, .. } = self;
        let close = FooterButton::new(tl!("Close").to_string(), DialogRole::Cancel);
        let out = FormDialog::new(tl!("Word count").to_string(), Bind::bound(options)).shaped(|d| d.buttons([close])).show(ui, |ui, _, o| {
            if key.is_none() && let Some(Err(error)) = shown {
                Text::new(error.clone()).show(ui);
                return true;
            }
            Checkbox::new(Bind::bound(&mut o.current_page)).label(tl!("Current page only").to_string()).show(ui);
            Checkbox::new(Bind::bound(&mut o.options.exclude_tables)).label(tl!("Exclude tagged tables").to_string()).show(ui);
            Checkbox::new(Bind::bound(&mut o.options.exclude_code)).label(tl!("Exclude tagged code blocks").to_string()).show(ui);
            Checkbox::new(Bind::bound(&mut o.options.exclude_images)).label(tl!("Exclude image/OCR text").to_string()).show(ui);
            Text::new(
                tl!("Exclusions use PDF tags. Untagged tables and code remain included. Image exclusion also removes invisible text.").to_string(),
            )
            .role(TypeRole::Caption)
            .shade(Shade::Muted)
            .show(ui);
            match shown {
                Some(Ok(report)) => {
                    for (label, value) in [
                        (tl!("Words"), report.total.words),
                        (tl!("Characters"), report.total.characters),
                        (tl!("Characters without spaces"), report.total.characters_without_spaces),
                        (tl!("Pages counted"), report.pages.len()),
                    ] {
                        let value = value.to_string();
                        Field::new(label.to_string()).value(value.clone()).show(ui, |ui, _| {
                            Text::new(value.clone()).show(ui);
                        });
                    }
                    if report.pages_without_text > 0 {
                        Text::new(tl!("Some pages contain no countable text. Scanned images need OCR first.").to_string()).show(ui);
                    }
                    if report.unclassified_structure {
                        Text::new(tl!("Some content could not be classified safely and was kept in the count.").to_string()).show(ui);
                    }
                }
                Some(Err(error)) => {
                    Notice::new(Tone::Bad, error.clone()).layout(NoticeLayout::Inline).show(ui);
                }
                None => {
                    Row::new().show(ui, |ui, _| {
                        Spinner::new().show(ui);
                        let n = done.load(Ordering::Relaxed);
                        Text::new(crate::i18n::fmt(tl!("Counting words… {n} pages read"), &[("n", &n.to_string())])).show(ui);
                    });
                }
            }
            true
        });
        out.closed.map(|_| DialogResult::Cancel)
    }
}

impl PdfCraftApp {
    /// Opens Word count on the options the last one left, with its own one-lane queue on the host's jobs.
    pub(crate) fn open_word_count(&mut self) {
        // PLAN: assumed `self.wake: Wake` (pdf-hub-w1, `AppServices::wake`)
        let wake = self.wake.clone();
        let queue =
            JobQueue::new(self.host.jobs_shared().clone(), wake.clone(), QueueOptions { lanes: 1, name: "word-count", ..QueueOptions::default() });
        let dialog = WordCountDialog::new(self.word_count_options, queue, wake);
        crate::dialogs::host::open(self, dialog);
    }

    /// Feeds the open Word count dialog the active document and takes the count's result.
    pub(crate) fn word_count_pass(&mut self, ctx: &egui::Context) {
        let Some(options) = self.dialogs.get::<WordCountDialog>().map(|d| d.options) else { return };
        self.word_count_options = options;
        let Some((view, id)) = self.active_ids() else {
            self.dialogs.cancel::<WordCountDialog>(ctx);
            return;
        };
        let current = self.views[view].current();
        let Some(doc) = self.session.get(id) else {
            self.dialogs.cancel::<WordCountDialog>(ctx);
            return;
        };
        let Some(dialog) = self.dialogs.get_mut::<WordCountDialog>() else { return };
        if let Err(error) = doc.check_copying() {
            dialog.refuse(error.to_string());
            return;
        }
        let key = Key { doc: id, generation: doc.edit_generation(), page: options.current_page.then_some(current), options: options.options };
        dialog.update(key, || doc.export_source());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft::host::ManualJobs;
    use egui_kittest::{
        Harness,
        kittest::{NodeT as _, Queryable},
    };

    /// An app whose background jobs wait on `jobs` until the test runs them.
    fn app_on(jobs: Arc<ManualJobs>) -> PdfCraftApp {
        let info = craft::host::AppInfo { id: "pdfcraft", name: "PdfCraft", version: env!("CARGO_PKG_VERSION") };
        let host = craft::host::Host::builder(info).jobs(jobs).build();
        let (intake, _) = craft::platform::open::FileIntake::new(Wake::none());
        let clipboard = craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::EventClipboard::new(|| 0, |_| Ok(0)));
        let watch = craft::platform::watch::FileWatch::new(Wake::none());
        let services = crate::AppServices::new(Wake::none(), clipboard, None, None, intake, watch);
        PdfCraftApp::with_host(Arc::new(host), services)
    }

    /// The figure in the Words row of the open dialog: the value its readout field reports.
    fn words_shown(h: &Harness<'_, PdfCraftApp>) -> String {
        h.get_by_role_and_label(egui::accesskit::Role::Group, "Words").value().expect("the Words row reports a figure")
    }

    #[test]
    fn word_count_dialog_counts_changes_pages_and_closes() {
        let jobs = Arc::new(ManualJobs::new());
        let bytes = pdfcraft_engine::Session::new().create_from_text("Words", "Hello world\u{c}Another three words").unwrap();
        let build_jobs = jobs.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let mut app = app_on(build_jobs);
                app.set_option(&cc.egui_ctx, "language", "en").unwrap();
                app.open_bytes(&egui::Context::default(), "words.pdf", None, bytes.as_ref().clone()).unwrap();
                app.run_command(&cc.egui_ctx, "file.word_count");
                app
            }
        });
        // One frame submits the count, the job runs, the next frames take its result.
        let settle = |h: &mut Harness<'_, PdfCraftApp>| {
            h.run_steps(1);
            jobs.run_all();
            h.run_steps(2);
            assert!(h.state().dialogs.get::<WordCountDialog>().is_some_and(WordCountDialog::finished), "the count finished");
        };
        settle(&mut h);
        assert_eq!(words_shown(&h), "5");
        h.get_by_label("Current page only").click();
        settle(&mut h);
        assert_eq!(words_shown(&h), "2");
        let ctx_h = h.ctx.clone();
        h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::DeletePages { pages: vec![0] });
        settle(&mut h);
        assert_eq!(words_shown(&h), "3");
        h.query_all_by_role_and_label(egui::accesskit::Role::Button, "Close").last().expect("the dialog's Close").click();
        h.run_steps(2);
        assert!(!h.state().modal_open());
        assert!(h.state().dialogs.get::<WordCountDialog>().is_none(), "closing drops the dialog and its queue");
    }
}
