//! The Print dialog (Acrobat's File ▸ Print, execution plan M10.5): printer, copies, grayscale;
//! pages to print (all, current, range with labels, the selected pages; odd/even, reverse); page sizing & handling
//! (Size, Poster, Multiple, Booklet); orientation; comments & forms; and a live preview of the
//! sheets. Printing sends the print-ready PDF to the system spooler; "Save as PDF" writes it.

use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use craft::host::queue::{JobQueue, Priority, QueueOptions, QueueOrder};
use craft::host::{Jobs, Wake};
use craft::print::ui::HostedPrint;
use pdfcraft_engine::print::{self, Binding, BookletSubset, Content, Layout, Orientation, PageOrder, SizeMode, Subset, spool};

use crate::PdfCraftApp;

mod body;
mod dialog;
mod draft;
mod setups;
mod sheet;

pub use draft::{CUSTOM_SCALE, Handling, IMAGE_DPI, PrintDraft, Which};
pub use setups::PrinterSetup;
pub(crate) use setups::{from_json as setups_from_json, to_json as setups_to_json};

/// Label printing reads the printed area of at most this many pages (each is a quick render on
/// the UI thread); further pages print whole.
pub(crate) const LABEL_CROP_PAGES: usize = 50;
/// Two-sided reads running at once. A driver Windows can't reach holds its read for about 48 s,
/// so picking printer after printer could otherwise start a thread for each; past this, the
/// printer picked is asked once a read finishes.
#[cfg(not(target_arch = "wasm32"))]
const TWO_SIDED_READS: usize = 4;

/// Lists the printers off the UI thread (the desktop app sets [`PdfCraftApp::printer_source`]):
/// CUPS' driverless discovery can wait on the network. Without a source the dialog reads them as
/// it opens, so tests stay deterministic.
pub type PrinterSource = std::sync::Arc<dyn Fn() -> Vec<spool::Printer> + Send + Sync>;

/// Where a printer's driver options come from: the spooler's (`spool::printer_options_in`, in the
/// UI language) unless a test stands in. Called off the GUI thread.
pub type OptionsSource = std::sync::Arc<dyn Fn(&str) -> Vec<spool::PrinterOption> + Send + Sync>;

/// Where a printer's configured default paper comes from: the spooler's DEVMODE / driver settings
/// unless a test stands in.
pub type DefaultPaperSource = std::sync::Arc<dyn Fn(&str) -> Option<(String, (f64, f64))> + Send + Sync>;

/// Where a printer's supported paper sizes come from: the spooler driver capabilities
/// unless a test stands in.
pub type PrinterPapersSource = std::sync::Arc<dyn Fn(&str) -> Vec<spool::Paper> + Send + Sync>;

/// What takes a print job in place of the system spooler: a test's stand-in, which gets the
/// print-ready PDF and the job and answers as the spooler would. Called off the GUI thread.
pub type Spooler = std::sync::Arc<dyn Fn(&[u8], &spool::Job) -> Result<String, String> + Send + Sync>;

/// The system spooler's printers, as the desktop app's [`PrinterSource`]: its queues only, with
/// no driver asked anything, so the list never waits on a printer (#2192).
pub fn system_printers() -> PrinterSource {
    std::sync::Arc::new(spool::printer_queues)
}

/// Reads whether one printer's driver takes two-sided settings, off the UI thread, for the
/// printer picked (the desktop app sets [`PdfCraftApp::two_sided_source`]). The printer list
/// leaves it unknown: reading a driver Windows can't reach takes it about 48 s, so the list
/// doesn't wait for every printer's (#2192).
pub type TwoSidedSource = std::sync::Arc<dyn Fn(&str) -> Option<bool> + Send + Sync>;

/// The system spooler's answer, as the desktop app's [`TwoSidedSource`].
pub fn system_two_sided() -> TwoSidedSource {
    std::sync::Arc::new(spool::two_sided)
}

#[cfg(not(target_arch = "wasm32"))]
type PaperInfo = (Vec<spool::Paper>, Option<spool::Paper>);

/// A print job being spooled: the ticket its queue entry has, where it goes, and the sheets the
/// spooler has so far.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct RunningPrint {
    /// The key of the job in [`PrintTasks::jobs`].
    pub(crate) ticket: u64,
    /// The printer the job goes to.
    pub(crate) printer: String,
    /// `(sent, sheets)`, set by the job as the spooler takes each sheet.
    pub(crate) count: Arc<super::job_count::JobCount>,
}

/// The Print dialog's work in the background: the printer list, the per-printer reads and the
/// jobs being spooled, each on a queue of the host's jobs.
pub(crate) struct PrintTasks {
    /// The printer list being read; a list already queued is as fresh as a new one.
    #[cfg(not(target_arch = "wasm32"))]
    lists: JobQueue<(), Option<Vec<spool::Printer>>>,
    /// Driver options, by printer (`read_options_in_background`).
    #[cfg(not(target_arch = "wasm32"))]
    options: JobQueue<String, Vec<spool::PrinterOption>>,
    /// The papers and default paper, by printer (`read_papers_in_background`).
    #[cfg(not(target_arch = "wasm32"))]
    papers: JobQueue<String, PaperInfo>,
    /// Two-sided support, by printer; at most [`TWO_SIDED_READS`] run at once.
    #[cfg(not(target_arch = "wasm32"))]
    two_sided: JobQueue<String, Option<bool>>,
    /// The jobs being spooled, by ticket.
    #[cfg(not(target_arch = "wasm32"))]
    jobs: JobQueue<u64, Result<String, String>>,
    /// The jobs being spooled, oldest first.
    #[cfg(not(target_arch = "wasm32"))]
    running: Vec<RunningPrint>,
    /// The ticket the next job gets.
    #[cfg(not(target_arch = "wasm32"))]
    next_ticket: u64,
    /// Cached papers and default paper per printer so switching printers avoids re-querying.
    #[cfg(not(target_arch = "wasm32"))]
    papers_cache: std::collections::HashMap<String, PaperInfo>,
    /// The last printer list, which the dialog offers at once the next time it opens.
    pub(crate) listed: Vec<craft::print::ui::Printer>,
    /// Wakes the app when a job reports.
    pub(crate) wake: Wake,
    /// The browser print being drawn, a sheet a frame (`web_print`).
    #[cfg(target_arch = "wasm32")]
    web_job: Option<crate::web_print::WebPrintJob>,
    /// What went wrong after the browser print page was handed over, with the print-ready PDF
    /// to download instead.
    #[cfg(target_arch = "wasm32")]
    web_failed: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// The last browser print's PDF and name, for that fallback.
    #[cfg(target_arch = "wasm32")]
    web_last: Option<(String, std::sync::Arc<Vec<u8>>)>,
}

impl PrintTasks {
    /// The queues on the host's `jobs`; each finished job calls `wake`.
    pub(crate) fn new(jobs: Arc<dyn Jobs>, wake: Wake) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let options = |name| QueueOptions { name, ..QueueOptions::default() };
            Self {
                lists: JobQueue::new(jobs.clone(), wake.clone(), options("print-printers")),
                options: JobQueue::new(jobs.clone(), wake.clone(), options("print-options")),
                papers: JobQueue::new(jobs.clone(), wake.clone(), options("print-papers")),
                two_sided: JobQueue::new(
                    jobs.clone(),
                    wake.clone(),
                    // A read for each pick, in the order the picks came.
                    QueueOptions { lanes: TWO_SIDED_READS, name: "print-two-sided", order: QueueOrder::OldestFirst },
                ),
                jobs: JobQueue::new(jobs, wake.clone(), options("print-jobs")),
                running: Vec::new(),
                next_ticket: 0,
                papers_cache: std::collections::HashMap::new(),
                listed: Vec::new(),
                wake,
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            // A browser has no threads to queue on.
            let _ = jobs;
            Self { listed: Vec::new(), wake, web_job: None, web_failed: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())), web_last: None }
        }
    }
}

/// The printers to offer when no [`PrinterSource`] is set: the spooler's, or in a browser its
/// print dialog.
fn listed_printers() -> Vec<spool::Printer> {
    #[cfg(target_arch = "wasm32")]
    {
        vec![spool::Printer { name: crate::web_print::BROWSER_PRINTER.to_string(), default: true, two_sided: None, description: String::new() }]
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        spool::printer_queues()
    }
}

/// The printers as the dialog offers them. The browser's print dialog decides copies, two-sided
/// and colour itself, so its entry says so.
fn offered_printers(list: &[spool::Printer]) -> Vec<craft::print::ui::Printer> {
    list.iter()
        .map(|p| {
            let printer = craft::print::ui::Printer::new(p.name.clone()).label(printer_label(&p.name)).is_default(p.default);
            if p.name == crate::web_print::BROWSER_PRINTER {
                printer.own_dialog(tl!("The browser's print dialog decides copies, two-sided printing and colour."))
            } else {
                printer
            }
        })
        .collect()
}

/// The first list is still being read and the user hasn't chosen yet (#160): no printer is
/// selected only because none is known yet, so the dialog says it is looking rather than
/// offering Save as PDF, and its button waits. Printing then would have saved a PDF.
/// `discovering` is the dialog's own fact that the list is being read.
fn still_looking(d: &PrintDraft, discovering: bool) -> bool {
    discovering && d.printer.is_none() && !d.printer_picked
}

/// A printer's name as the dialog shows it.
fn printer_label(name: &str) -> String {
    if name == crate::web_print::BROWSER_PRINTER { crate::web_print::browser_printer_label().to_string() } else { name.to_string() }
}

/// The printer the dialog preselects: the one printed to last while it is listed (#2001), else
/// the system default, else the first.
fn default_printer(d: &PrintDraft, printers: &[craft::print::ui::Printer]) -> Option<String> {
    d.last_printer(printers).or_else(|| printers.iter().find(|p| p.default)).or(printers.first()).map(|p| p.name.clone())
}

/// Whether `printer` can still be chosen from `printers`: Save as PDF (`None`) always can.
fn offered(printer: Option<&str>, printers: &[craft::print::ui::Printer]) -> bool {
    printer.is_none_or(|name| printers.iter().any(|p| p.name == name))
}

/// What a started print left to do for the dialog's held OK.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintStart {
    /// The job was handed to the spooler, the browser print started or the download finished.
    Sent,
    /// Save as PDF is asking where to write; the pick answers the held OK.
    Picking,
}

impl PdfCraftApp {
    /// Label printing: find each printed page's printed area (a quick low-resolution render, as
    /// Fit Visible does) and which way its text runs. Only the pages being printed are read, at
    /// most [`LABEL_CROP_PAGES`] of them (the rest print whole); read again when they change.
    pub(crate) fn read_label_crops(&mut self, id: pdfcraft_engine::DocId) {
        let Some(doc) = self.session.get(id) else { return };
        let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        let mut pages = self.print_draft.settings(doc.info.pages.len(), &labels).map(|s| s.pages).unwrap_or_default();
        pages.sort_unstable();
        pages.dedup();
        pages.truncate(LABEL_CROP_PAGES);
        if self.print_draft.crop_pages.as_ref() == Some(&pages) {
            return;
        }
        let up = self.session.print_text_up(id, &pages);
        let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        self.print_draft.crops = doc
            .info
            .pages
            .iter()
            .enumerate()
            .map(|(page, p)| {
                let (w, h) = (f64::from(p.width), f64::from(p.height));
                if pages.binary_search(&page).is_err() || w <= 0.0 || h <= 0.0 {
                    return None;
                }
                let out = r.render(pdfcraft_render::RenderRequest {
                    page,
                    kind: pdfcraft_render::RequestKind::Pixels,
                    tile: None,
                    scale: (1000.0 / w.max(h) as f32).clamp(0.1, 4.0),
                    tag: 0,
                    preview: false,
                });
                let [u0, v0, u1, v1] = crate::zoom_snap::ink_bounds(out.width as usize, out.height as usize, &out.rgba)?;
                // Image rows run down from the top; display space runs up. Half a millimetre of slack.
                let pad = 1.5;
                let rect = [f64::from(u0) * w - pad, (1.0 - f64::from(v1)) * h - pad, f64::from(u1) * w + pad, (1.0 - f64::from(v0)) * h + pad];
                Some(print::Crop { rect, text_up: up.get(page).copied().unwrap_or(false) })
            })
            .collect();
        self.print_draft.crop_pages = Some(pages);
    }

    /// Remember a printer list the spooler gave: it is what the next open offers at once, and a
    /// list that says whether a driver takes two-sided settings answers that read.
    pub(crate) fn record_listed(&mut self, list: &[spool::Printer]) {
        self.print_tasks.listed = offered_printers(list);
        for p in list {
            // Only a known answer: an inserted `None` would stop `poll_two_sided` asking.
            if let Some(two_sided) = p.two_sided {
                self.print_draft.two_sided.insert(p.name.clone(), Some(two_sided));
            }
        }
    }

    /// Read the printer list in the background off the GUI thread. The offered list is sent on
    /// the returned channel for the dialog to take (`HostedPrint::listen`); `poll_print` records
    /// the spooler's own list. A job that died closes the channel.
    #[cfg(not(target_arch = "wasm32"))]
    fn list_printers(&mut self) -> std::sync::mpsc::Receiver<Vec<craft::print::ui::Printer>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let source = self.printer_source.clone();
        self.print_tasks.lists.submit((), Priority::VISIBLE, move |_| {
            let list = pdfcraft_engine::guard(|| match &source {
                Some(source) => source(),
                None => listed_printers(),
            })
            .ok()?;
            // The receiver may be gone (the dialog closed): nothing to report to then.
            let _ = tx.send(offered_printers(&list));
            Some(list)
        });
        rx
    }

    /// Read `printer`'s driver options off the GUI thread (#1818); `poll_print` picks them up.
    /// A read already under way for the same printer is left to finish.
    #[cfg(not(target_arch = "wasm32"))]
    fn read_options_in_background(&mut self, printer: String) {
        if self.print_tasks.options.contains(&printer) {
            return;
        }
        let source: OptionsSource = self.options_source.clone().unwrap_or_else(|| {
            // The driver's own names in the UI language, as it is when Properties… is clicked.
            let lang = crate::i18n::current().code();
            std::sync::Arc::new(move |p: &str| spool::printer_options_in(p, lang))
        });
        let name = printer.clone();
        // A read that died offers nothing; the panel says so rather than wait.
        self.print_tasks.options.submit(printer, Priority::VISIBLE, move |_| pdfcraft_engine::guard(|| source(&name)).unwrap_or_default());
    }

    /// The options read for `printer` arrived. A read that a later click superseded (another
    /// printer's is awaited) is dropped.
    #[cfg(not(target_arch = "wasm32"))]
    fn receive_options(&mut self, printer: String, options: Vec<spool::PrinterOption>) {
        let d = &mut self.print_draft;
        if d.driver_options_loading.as_deref() == Some(printer.as_str()) {
            d.driver_options_loading = None;
            d.driver_options = Some((printer, options));
        }
    }

    /// Read `printer`'s papers and default paper off the GUI thread (#5861); `poll_print` picks them up.
    /// A read already under way for the same printer is left to finish.
    #[cfg(not(target_arch = "wasm32"))]
    fn read_papers_in_background(&mut self, printer: String) {
        if let Some((papers, default_paper)) = self.print_tasks.papers_cache.get(&printer).cloned() {
            let d = &mut self.print_draft;
            if d.printer.as_deref() == Some(printer.as_str()) {
                d.apply_printer_papers(papers);
                if !d.paper_overridden {
                    d.apply_default_paper(default_paper);
                }
            }
        }
        self.print_draft.papers_loading = Some(printer.clone());
        if self.print_tasks.papers.contains(&printer) {
            return;
        }
        let papers_source = self.printer_papers_source.clone();
        let default_source = self.default_paper_source.clone();
        let name = printer.clone();
        self.print_tasks.papers.submit(printer, Priority::VISIBLE, move |_| {
            pdfcraft_engine::guard(|| match (&papers_source, &default_source) {
                (None, None) => spool::printer_paper_info(&name),
                (papers_s, def_s) => {
                    let papers = match papers_s {
                        Some(s) => s(&name),
                        None => spool::printer_papers(&name),
                    };
                    let def = match def_s {
                        Some(s) => s(&name),
                        None => spool::printer_default_paper(&name),
                    };
                    (papers, def)
                }
            })
            // A read that died leaves the paper list as it is.
            .unwrap_or_default()
        });
    }

    /// The papers and default paper read for `printer` arrived.
    #[cfg(not(target_arch = "wasm32"))]
    fn receive_papers(&mut self, printer: String, papers: Vec<spool::Paper>, default_paper: Option<spool::Paper>) {
        if !papers.is_empty() || default_paper.is_some() {
            self.print_tasks.papers_cache.insert(printer.clone(), (papers.clone(), default_paper.clone()));
        }
        let d = &mut self.print_draft;
        if d.papers_loading.as_deref() == Some(printer.as_str()) {
            d.papers_loading = None;
        }
        if d.printer.as_deref() == Some(printer.as_str()) {
            if !papers.is_empty() {
                d.apply_printer_papers(papers);
            }
            if !d.paper_overridden && default_paper.is_some() {
                d.apply_default_paper(default_paper);
            }
        }
    }

    /// A fresh printer list arrived: record it, and move a printer that is no longer offered (or
    /// was never chosen) to the default.
    #[cfg(not(target_arch = "wasm32"))]
    fn receive_list(&mut self, list: Vec<spool::Printer>) {
        self.record_listed(&list);
        let printers = self.print_tasks.listed.clone();
        let d = &mut self.print_draft;
        // A picked printer that is no longer listed falls back to the default too.
        if d.printer_picked && offered(d.printer.as_deref(), &printers) {
            return;
        }
        d.printer_picked = false;
        let printer = default_printer(d, &printers);
        if printer == d.printer {
            return;
        }
        d.printer = printer;
        d.apply_setup();
        d.reset_unoverridden_custom_paper();
        d.apply_printer_papers(Vec::new());
        if let Some(p) = d.printer.clone() {
            self.read_papers_in_background(p);
        }
        self.show_chosen_printer();
    }

    /// Pick up a printer list, driver options or a spooled job that finished (each frame).
    pub(crate) fn poll_print(&mut self) {
        #[cfg(target_arch = "wasm32")]
        self.poll_web_print();
        #[cfg(not(target_arch = "wasm32"))]
        {
            while let Some(done) = self.print_tasks.papers.try_recv() {
                let (papers, default_paper) = done.result;
                self.receive_papers(done.key, papers, default_paper);
            }
            while let Some(done) = self.print_tasks.options.try_recv() {
                self.receive_options(done.key, done.result);
            }
            while let Some(done) = self.print_tasks.lists.try_recv() {
                // A source that died lists nothing new; the dialog keeps what it has.
                if let Some(list) = done.result {
                    self.receive_list(list);
                }
            }
            self.poll_two_sided();
            self.poll_jobs();
        }
    }

    /// Take the jobs that finished, and keep the progress card on the oldest one still being
    /// spooled: it moves when the job's count does, and goes when no job is left.
    #[cfg(not(target_arch = "wasm32"))]
    fn poll_jobs(&mut self) {
        let mut ended = false;
        while let Some(done) = self.print_tasks.jobs.try_recv() {
            let Some(at) = self.print_tasks.running.iter().position(|r| r.ticket == done.key) else { continue };
            let finished = self.print_tasks.running.remove(at);
            ended = true;
            self.report_print_job(&finished.printer, done.result);
        }
        match self.print_tasks.running.first() {
            Some(oldest) => {
                let pending = oldest.count.take_change();
                // A job that just became the oldest shows what it has reported so far, not zero.
                // PLAN: assumed `JobCount::read(&self) -> (usize, usize)` (pdf-print-ocr-w6).
                let change = if ended { Some(oldest.count.read()) } else { pending };
                let printer = oldest.printer.clone();
                if change.is_some() {
                    self.show_print_progress(&printer, change);
                }
            }
            None if ended => crate::notices::end_progress(self, crate::notices::ProgressKey::Print),
            None => {}
        }
    }

    /// The progress card for a job to `printer`, counting its sheets as they reach the spooler.
    #[cfg(not(target_arch = "wasm32"))]
    fn show_print_progress(&mut self, printer: &str, sheets: Option<(usize, usize)>) {
        let (label, fraction) = match sheets {
            Some((sent, sheets)) if sheets > 0 => {
                // The sheet on its way: one past those sent, until the last is.
                let sheet = sent.saturating_add(1).min(sheets).to_string();
                let label = crate::i18n::fmt(
                    tl!("Sending sheet {s} of {n} to {printer}…"),
                    &[("s", &sheet), ("n", &sheets.to_string()), ("printer", printer)],
                );
                (label, sent as f32 / sheets as f32)
            }
            _ => (crate::i18n::fmt(tl!("Sending to {printer}…"), &[("printer", printer)]), 0.0),
        };
        crate::notices::set_progress(
            self,
            crate::notices::ProgressKey::Print,
            crate::notices::ProgressNotice { label, fraction, cancellable: false },
        );
    }

    /// Pick up two-sided answers that arrived, and ask for the picked printer's when the list
    /// doesn't say, nobody has asked yet and a lane is free: a read never waits in the queue for
    /// a printer that is no longer the one picked.
    #[cfg(not(target_arch = "wasm32"))]
    fn poll_two_sided(&mut self) {
        while let Some(done) = self.print_tasks.two_sided.try_recv() {
            self.print_draft.two_sided.insert(done.key, done.result);
        }
        if !self.dialogs.is_open::<HostedPrint>() {
            return;
        }
        let Some(source) = self.two_sided_source.clone() else { return };
        let Some(name) = self.print_draft.printer.clone() else { return };
        let queue = &self.print_tasks.two_sided;
        // A read is out from its submit until its answer arrives, whether or not its lane has started it yet.
        let out = queue.counts();
        if !self.print_tasks.listed.iter().any(|p| p.name == name)
            || self.print_draft.two_sided.contains_key(&name)
            || queue.contains(&name)
            || out.queued + out.running >= TWO_SIDED_READS
        {
            return;
        }
        let asked = name.clone();
        queue.submit(name, Priority::VISIBLE, move |_| pdfcraft_engine::guard(|| source(&asked)).unwrap_or(None));
    }

    /// Report the outcome of a finished print job to the user.
    fn report_print_job(&mut self, printer: &str, result: Result<String, String>) {
        match result {
            Ok(msg) if msg.is_empty() => self.notify(crate::i18n::fmt(tl!("Sent to {printer}"), &[("printer", printer)])),
            Ok(msg) => self.notify(crate::i18n::fmt(tl!("Sent to {printer}: {msg}"), &[("printer", printer), ("msg", &msg)])),
            Err(e) => self.notify_error(e),
        }
    }

    /// Print (or save) with the dialog's settings. `Err` is the reason the held OK shows. Save as
    /// PDF without a preset path answers [`PrintStart::Picking`] once the save picker is showing;
    /// the file is written when the user has chosen where, and that write answers the held OK.
    pub fn print_now(&mut self, ctx: &egui::Context) -> Result<PrintStart, String> {
        let discovering = self.dialogs.get::<HostedPrint>().is_some_and(|p| p.state().discovering());
        if still_looking(&self.print_draft, discovering) || self.print_draft.papers_loading() {
            return Err(tl!("Looking for printers…").to_string());
        }
        // What's typed in a form field is part of what's printed (#166).
        if !self.commit_form_typing(ctx) {
            return Err(tl!("A form field has text that can't be kept yet.").to_string());
        }
        let Some((_, id)) = self.active_ids() else { return Err(tl!("There is no document to print.").to_string()) };
        // A form's Will Print script runs first: what it changes is printed.
        if self.session.get(id).is_some_and(|d| d.allows_printing()) {
            let before = self.session.get(id).map(|d| d.edit_generation());
            self.run_document_actions(ctx, id, pdfcraft_engine::js::DocumentScriptEvent::WillPrint);
            // What it showed may lie outside the printed areas Label printing found: find them again.
            if self.session.get(id).map(|d| d.edit_generation()) != before && self.print_draft.handling == Handling::Label {
                self.print_draft.crop_pages = None;
                self.read_label_crops(id);
            }
        }
        let Some(doc) = self.session.get(id) else { return Err(tl!("There is no document to print.").to_string()) };
        let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        // `settings` words its errors for the person already.
        let settings = self.print_draft.settings(doc.info.pages.len(), &labels)?;
        let name = doc.name.clone();
        let bytes = self.session.print_pdf(id, &settings).map_err(|e| e.to_string())?;
        let to_printer = self.print_draft.printer.is_some();
        let sent = match self.print_draft.printer.clone() {
            #[cfg(target_arch = "wasm32")]
            Some(printer) if printer == crate::web_print::BROWSER_PRINTER => {
                let sizes: Vec<(f64, f64)> = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
                let sheets = print::layout(&sizes, &settings).map_err(|e| e.to_string())?;
                self.print_in_browser(name, bytes, sheets.iter().map(|s| s.size).collect())
            }
            Some(printer) => {
                let mut job = self.print_draft.job(&name);
                // CUPS is told which way the sheets run (#2713), unless a driver choice already
                // says; Windows' print ticket already is.
                let sizes: Vec<(f64, f64)> = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
                if let Ok(sheets) = print::layout(&sizes, &settings) {
                    print::add_cups_orientation(&mut job.options, &sheets);
                }
                self.send_to_printer(printer, bytes, job)
            }
            None => self.save_print_pdf(&name, bytes),
        }?;
        // What this printer printed with is what it starts with next time (#2001).
        if to_printer {
            self.print_draft.remember_setup();
        }
        Ok(sent)
    }

    /// Spool a job off the UI thread: on Windows every sheet is rendered at the printer's
    /// resolution first, which can take minutes for a long document. The dialog closes at once,
    /// the progress card counts the sheets as they reach the spooler, and the spooler's answer
    /// arrives as a notice.
    #[cfg(not(target_arch = "wasm32"))]
    fn send_to_printer(&mut self, printer: String, bytes: Vec<u8>, mut job: spool::Job) -> Result<PrintStart, String> {
        let ticket = self.print_tasks.next_ticket;
        self.print_tasks.next_ticket = ticket.wrapping_add(1);
        let count = Arc::new(super::job_count::JobCount::default());
        let progress = count.clone();
        let wake = self.print_tasks.wake.clone();
        let spooler = self.spooler.clone();
        self.print_tasks.jobs.submit(ticket, Priority::VISIBLE, move |_| {
            pdfcraft_engine::guard(|| {
                // On Windows grayscale makes the pages grey (#5806) and large pictures become tiles
                // (#5131): work on the whole file, kept off the UI thread with the rest.
                // A GDI (v3) driver gets transparent sheets flattened (#6155, #5838, #5940); XPS (v4)
                // drivers print them as they are.
                let flatten = job.printer.as_deref().is_some_and(spool::gdi_driver);
                let bytes = pdfcraft_engine::spooler::spooler_copy(bytes, &mut job, flatten);
                match &spooler {
                    Some(submit) => submit(&bytes, &job),
                    None => spool::submit_with_progress(&bytes, &job, &mut |sent, sheets| {
                        progress.set(sent, sheets);
                        wake.wake();
                    })
                    .map_err(|e| e.to_string()),
                }
            })
            .unwrap_or_else(|_| Err("the print job stopped unexpectedly".into()))
        });
        self.print_tasks.running.push(RunningPrint { ticket, printer: printer.clone(), count });
        self.show_print_progress(&printer, None);
        Ok(PrintStart::Sent)
    }

    /// A browser has no spooler: `submit` answers at once.
    #[cfg(target_arch = "wasm32")]
    fn send_to_printer(&mut self, printer: String, bytes: Vec<u8>, job: spool::Job) -> Result<PrintStart, String> {
        let msg = spool::submit(&bytes, &job).map_err(|e| e.to_string())?;
        self.report_print_job(&printer, Ok(msg));
        Ok(PrintStart::Sent)
    }

    /// Print through the browser's print dialog: start drawing `sheets` (the pages of the
    /// print-ready `bytes`), a sheet a frame in [`PdfCraftApp::poll_print`]. A print still being
    /// prepared is replaced. The dialog closes at once.
    #[cfg(target_arch = "wasm32")]
    fn print_in_browser(&mut self, name: String, bytes: Vec<u8>, sheets: Vec<(f64, f64)>) -> Result<PrintStart, String> {
        let job = crate::web_print::WebPrintJob::new(bytes, sheets, name)?;
        let n = job.total().to_string();
        self.print_tasks.web_job = Some(job);
        self.notify_fmt("Preparing {n} sheets for printing…", &[("n", &n)]);
        self.print_tasks.wake.wake();
        Ok(PrintStart::Sent)
    }

    /// Draw the next sheet of the browser print, and hand the finished page to the browser.
    #[cfg(target_arch = "wasm32")]
    fn poll_web_print(&mut self) {
        let failed: Vec<String> = self.print_tasks.web_failed.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        if let Some(e) = failed.into_iter().next() {
            self.web_print_failed(&e);
        }
        let Some(job) = self.print_tasks.web_job.as_mut() else { return };
        if !job.finished() {
            if let Err(e) = job.step(&mut crate::web_print::blob_url) {
                self.print_tasks.web_job = None;
                self.notify_error(e);
                return;
            }
            let (done, total) = (job.done().to_string(), job.total().to_string());
            self.notify_fmt("Preparing sheet {done} of {total} for printing…", &[("done", &done), ("total", &total)]);
            self.print_tasks.wake.wake();
            return;
        }
        let Some(mut job) = self.print_tasks.web_job.take() else { return };
        let html = job.html();
        let urls = job.take_urls();
        self.print_tasks.web_last = Some((job.name.clone(), job.pdf.clone()));
        match crate::web_print::print_in_iframe(&html, urls, self.print_tasks.web_failed.clone(), self.wake.clone()) {
            Ok(()) => self.notify_tr("Opening the browser's print dialog…"),
            Err(e) => self.web_print_failed(&e),
        }
    }

    /// The browser wouldn't print (a sandboxed frame, a picture it couldn't decode): offer the
    /// print-ready PDF as a download instead, so the work isn't lost.
    #[cfg(target_arch = "wasm32")]
    fn web_print_failed(&mut self, error: &str) {
        let Some((name, pdf)) = self.print_tasks.web_last.take() else {
            self.notify_error(error);
            return;
        };
        let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
        let file = format!("{stem} (print).pdf");
        // PLAN: assumed `Files::write(&self, path: &Path, bytes: &[u8]) -> Result<(), E: Display>` (SH-C35; a browser downloads under the file name).
        match self.host.files().write(std::path::Path::new(&file), &pdf, craft::host::WriteIntent::SaveAs) {
            // `false`: no host page took the bytes, so the browser downloaded them.
            Ok(false) => {
                self.notify_fmt("Couldn't print from the browser ({e}); downloaded {name} to print instead", &[("e", error), ("name", &file)])
            }
            Ok(true) | Err(_) => self.notify_error(error),
        }
    }

    /// Print ▸ Save as PDF on the desktop: ask where, and write on a later frame. The write
    /// answers the dialog's held OK; a cancelled pick gives the OK back with the settings kept.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_print_pdf(&mut self, name: &str, bytes: Vec<u8>) -> Result<PrintStart, String> {
        let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
        let request = craft::host::PickRequest::save("print").file_name(format!("{stem} (print).pdf")).filter("PDF", &["pdf"]);
        self.pick_file(request, None, move |app, _, picked| {
            let answer = match picked {
                // A picker that can't open refuses the held OK; only a cancel gives it back.
                craft::host::Picked::Unavailable(reason) => Err(reason),
                picked => match picked.first_path() {
                    None => {
                        if let Some(print) = app.dialogs.get_mut::<HostedPrint>() {
                            print.release_ok();
                        }
                        return;
                    }
                    Some(path) => match app.host.files().write(&path, &bytes, craft::host::WriteIntent::SaveAs) {
                        Ok(_) => {
                            app.notify_fmt("Saved the print-ready PDF to {path}", &[("path", &path.display().to_string())]);
                            Ok(())
                        }
                        Err(e) => {
                            let reason = e.to_string();
                            app.notify_fmt("Could not save: {e}", &[("e", &reason)]);
                            Err(reason)
                        }
                    },
                },
            };
            if let Some(print) = app.dialogs.get_mut::<HostedPrint>() {
                print.answer_ok(answer.map_err(Into::into));
            }
        });
        Ok(PrintStart::Picking)
    }

    /// Print ▸ Save as PDF in a browser: download it, as Save does (#170). There is no folder to
    /// choose and no file system to write to.
    #[cfg(target_arch = "wasm32")]
    fn save_print_pdf(&mut self, name: &str, bytes: Vec<u8>) -> Result<PrintStart, String> {
        let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
        let file = format!("{stem} (print).pdf");
        match self.host.files().write(std::path::Path::new(&file), &bytes, craft::host::WriteIntent::SaveAs) {
            Ok(taken_by_host) => {
                // Only a browser download is announced here; a host page that took the bytes says its own.
                if !taken_by_host {
                    self.notify_fmt("Downloaded {name}", &[("name", &file)]);
                }
                Ok(PrintStart::Sent)
            }
            Err(e) => Err(crate::i18n::fmt(tl!("Couldn't download {name}: {e}"), &[("name", &file), ("e", &e.to_string())])),
        }
    }
}

/// The longest side, in pixels, of one page's print-preview raster (also kept within the GPU's
/// `max_texture_side`): an Actual-size print of a large page would otherwise ask for a raster
/// bigger than a texture can be.
pub(crate) const PREVIEW_MAX_SIDE: f32 = 4096.0;
/// The most bytes of print-preview rasters kept at once. The current sheet comes first, then the
/// next and the previous one; a neighbour that doesn't fit isn't rendered ahead.
pub(crate) const PREVIEW_BYTES: f64 = 128.0 * 1024.0 * 1024.0;

/// Pages on the current sheet and its neighbours, each with the device pixels per point that
/// fill the preview's paper box (so the picture is not a stretched thumbnail), nearest sheet first
/// and within [`PREVIEW_MAX_SIDE`] (and `max_side`, the GPU's texture limit) and [`PREVIEW_BYTES`].
/// `paper_px` is the box the dialog drew the paper in last pass (`PrintPreviewOutput::paper_px`);
/// before the first drawn pass there is none, and no raster is asked for.
// PLAN: pdf-print-ocr-w4's callers pass `PrintPreviewOutput::paper_px` (F-3); assumed to be an `egui::Vec2` in points.
pub(crate) fn preview_rasters(
    d: &PrintDraft,
    sizes: &[(f64, f64)],
    labels: &[String],
    ppp: f32,
    max_side: f32,
    paper_px: Option<egui::Vec2>,
) -> Vec<(usize, f32)> {
    let Some(pane) = paper_px.filter(|p| p.x.is_finite() && p.y.is_finite() && p.x >= 1.0 && p.y >= 1.0) else { return Vec::new() };
    let Ok(settings) = d.settings(sizes.len(), labels) else { return Vec::new() };
    let Ok(sheets) = print::layout(sizes, &settings) else { return Vec::new() };
    if sheets.is_empty() {
        return Vec::new();
    }
    let ppp = if ppp.is_finite() { ppp.max(1.0) } else { 1.0 };
    let side = if max_side.is_finite() && max_side >= 1.0 { max_side.min(PREVIEW_MAX_SIDE) } else { PREVIEW_MAX_SIDE };
    let i = d.sheet.min(sheets.len() - 1);
    // Nearest first: the sheet on screen, then the next one, then the previous one.
    let order = [Some(i), i.checked_add(1).filter(|n| *n < sheets.len()), i.checked_sub(1)];
    let mut out: Vec<(usize, f32, f64)> = Vec::new();
    let mut bytes = 0.0f64;
    for sheet in order.into_iter().flatten().filter_map(|s| sheets.get(s)) {
        let (sw, sh) = (sheet.size.0 as f32, sheet.size.1 as f32);
        if !(sw.is_finite() && sh.is_finite()) || sw < 1.0 || sh < 1.0 {
            continue;
        }
        let k = (pane.x / sw).min(pane.y / sh);
        for pl in &sheet.placed {
            let [a, b, _, _, _, _] = pl.matrix.0;
            let placed = (a.hypot(b) as f32) * k * ppp;
            let Some(&(pw, ph)) = sizes.get(pl.page) else { continue };
            let long_pt = pw.max(ph) as f32;
            if !placed.is_finite() || placed < 0.05 || !long_pt.is_finite() || long_pt < 1.0 {
                continue;
            }
            let scale = placed.min(64.0).min(side / long_pt);
            let cost = (pw * f64::from(scale)).ceil() * (ph * f64::from(scale)).ceil() * 4.0;
            if let Some(slot) = out.iter_mut().find(|(page, _, _)| *page == pl.page) {
                if scale > slot.1 && bytes - slot.2 + cost <= PREVIEW_BYTES {
                    bytes += cost - slot.2;
                    (slot.1, slot.2) = (scale, cost);
                }
            } else if out.len() < 48 && (out.is_empty() || bytes + cost <= PREVIEW_BYTES) {
                bytes += cost;
                out.push((pl.page, scale, cost));
            }
        }
    }
    out.into_iter().map(|(page, scale, _)| (page, scale)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A paper box as the dialog might draw it.
    const PANE: Option<egui::Vec2> = Some(egui::vec2(296.0, 356.0));

    #[test]
    fn current_page_preview_uses_positions_and_selection_guards() {
        for labels in [["i", "ii", "iii", "1", "2"], ["1", "1", "1", "1", "1"], ["1", "2", "3", "4", "5"]] {
            let labels: Vec<String> = labels.into_iter().map(str::to_owned).collect();
            for current_page in 0..5 {
                let draft = PrintDraft { which: Which::Current, current_page, ..Default::default() };
                let rasters = preview_rasters(&draft, &[(200.0, 300.0); 5], &labels, 1.0, 8192.0, PANE);
                assert_eq!(rasters.iter().map(|(page, _)| *page).collect::<Vec<_>>(), vec![current_page]);
            }
        }
        let labels: Vec<String> = ["i", "ii", "iii", "1", "2"].into_iter().map(str::to_owned).collect();
        let mut draft = PrintDraft { which: Which::Selected, selected: vec![0, 3], ..Default::default() };
        assert_eq!(draft.settings(5, &labels).unwrap().pages, vec![0, 3]);
        draft.which = Which::All;
        assert_eq!(draft.settings(5, &labels).unwrap().pages, vec![0, 1, 2, 3, 4]);
        draft.which = Which::Current;
        draft.reverse = true;
        draft.subset = Subset::Odd;
        assert_eq!(draft.settings(5, &labels).unwrap().pages, vec![0]);
        draft.subset = Subset::Even;
        assert!(draft.settings(5, &labels).is_err());
        draft.subset = Subset::All;
        for (count, current) in [(0, 0), (5, 5), (5, usize::MAX)] {
            draft.current_page = current;
            assert!(draft.settings(count, &labels).is_err());
        }
    }

    /// #5806: with Print as image, grayscale makes the pictures grey, so the job doesn't ask the
    /// driver for it as well; a driver that keeps no grayscale setting then still prints.
    #[test]
    fn grey_pictures_dont_ask_the_driver_for_grayscale() {
        let vector = PrintDraft { grayscale: true, ..PrintDraft::default() };
        assert!(vector.job("a.pdf").grayscale, "vector pages: the driver makes them grey");
        let pictures = PrintDraft { grayscale: true, as_image: true, ..PrintDraft::default() };
        assert!(!pictures.job("a.pdf").grayscale, "grey pictures: nothing for the driver to do");
        assert!(pictures.settings(1, &[]).unwrap().raster.is_some_and(|r| r.grayscale), "grey pictures: the pages are rendered grey");
        assert!(!PrintDraft::default().job("a.pdf").grayscale);
    }

    #[test]
    fn composed_sheets_keep_exact_scaling_and_the_dialogs_duplex_choice() {
        let draft = PrintDraft { duplex: Some(spool::Duplex::LongEdge), ..PrintDraft::default() };
        let job = draft.job("report.pdf");
        assert_eq!(job.scaling, spool::Scaling::Exact);
        assert_eq!(job.duplex, Some(spool::Duplex::LongEdge));
        let off = PrintDraft { duplex: Some(spool::Duplex::Off), ..PrintDraft::default() };
        assert_eq!(off.job("report.pdf").duplex, Some(spool::Duplex::Off));
    }

    /// Untouched, Two-sided sends nothing, so the duplex set in the printer's Properties applies
    /// (#2422); cut and stack still asks for single-sided sheets.
    #[test]
    fn two_sided_follows_the_printer_until_chosen() {
        assert_eq!(PrintDraft::default().duplex, None);
        assert_eq!(PrintDraft::default().job("report.pdf").duplex, None);
        let mut cut = PrintDraft { handling: Handling::Multiple, order: PageOrder::CutStack, ..PrintDraft::default() };
        assert_eq!(cut.job("report.pdf").duplex, Some(spool::Duplex::Off));
        assert!(cut.settings(4, &[]).is_ok(), "the printer's default doesn't block cut and stack");
        cut.duplex = Some(spool::Duplex::ShortEdge);
        assert!(cut.settings(4, &[]).is_err());
    }

    /// #3208: long or short edge goes to a printer whose driver doesn't take two-sided settings
    /// as no duplex setting at all, so the driver keeps its own instead of refusing the job; Off
    /// still reaches it, a printer that can or whose support is unknown still gets the choice,
    /// and a stale long-edge choice doesn't block cut and stack. The drivers are unasked when the
    /// printers are listed (#3554); each answer sits in `two_sided`.
    #[test]
    fn duplex_is_not_sent_to_a_printer_without_two_sided() {
        let draft = |name: &str, duplex| PrintDraft {
            two_sided: [("Office".to_string(), Some(false)), ("Duplexer".to_string(), Some(true))].into_iter().collect(),
            printer: Some(name.into()),
            duplex,
            ..PrintDraft::default()
        };
        for duplex in [spool::Duplex::LongEdge, spool::Duplex::ShortEdge] {
            assert_eq!(draft("Office", Some(duplex)).job("report.pdf").duplex, None, "{duplex:?} to a printer without two-sided");
            assert_eq!(draft("Duplexer", Some(duplex)).job("report.pdf").duplex, Some(duplex));
            assert_eq!(draft("Network", Some(duplex)).job("report.pdf").duplex, Some(duplex), "unknown support keeps the choice");
        }
        assert_eq!(draft("Office", Some(spool::Duplex::Off)).job("report.pdf").duplex, Some(spool::Duplex::Off), "Off still reaches it");
        assert_eq!(draft("Office", None).job("report.pdf").duplex, None);
        // A long-edge choice left from another printer doesn't block cut and stack on this one.
        let mut cut = draft("Office", Some(spool::Duplex::LongEdge));
        (cut.handling, cut.order) = (Handling::Multiple, PageOrder::CutStack);
        assert!(cut.settings(4, &[]).is_ok());
        assert_eq!(cut.job("report.pdf").duplex, Some(spool::Duplex::Off));
        cut.printer = Some("Duplexer".into());
        assert!(cut.settings(4, &[]).is_err(), "still refused where it would print two-sided");
    }

    /// #5772: a booklet's both sides ask the printer for the short edge, unless its driver doesn't
    /// take two-sided settings: it would refuse the job, so its own setting (manual two-sided, say)
    /// prints it. One side on its own stays one-sided.
    #[test]
    fn a_booklet_leaves_two_sided_to_a_driver_that_keeps_its_own() {
        // The answers for the printers picked arrive in `two_sided`, and the network printer's never came.
        let mut d = PrintDraft {
            two_sided: [("Epson".to_string(), Some(false)), ("Laser".to_string(), Some(true))].into_iter().collect(),
            handling: Handling::Booklet,
            ..PrintDraft::default()
        };
        for (name, both) in [("Epson", None), ("Laser", Some(spool::Duplex::ShortEdge)), ("Network", Some(spool::Duplex::ShortEdge))] {
            d.printer = Some(name.into());
            d.booklet_subset = BookletSubset::BothSides;
            assert_eq!(d.job("a.pdf").duplex, both, "{name}");
            d.booklet_subset = BookletSubset::FrontOnly;
            assert_eq!(d.job("a.pdf").duplex, Some(spool::Duplex::Off), "{name}");
        }
        // Save as PDF has no driver: as before.
        d.printer = None;
        d.booklet_subset = BookletSubset::BothSides;
        assert_eq!(d.job("a.pdf").duplex, Some(spool::Duplex::ShortEdge));
    }

    /// A job being spooled holds the print progress card, counting its sheets as the spooler
    /// reports them; the card goes when the job ends and its answer arrives as a notice.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_progress_card_counts_the_sheets_of_a_print_job() {
        let jobs = Arc::new(craft::host::ManualJobs::new());
        let mut app = PdfCraftApp::for_unit_tests();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.print_tasks = PrintTasks::new(jobs.clone(), Wake::none());
        // The notices the app queued are posted into one context, as a pass would post them.
        let ctx = egui::Context::default();
        let card = |app: &mut PdfCraftApp| {
            crate::notices::post_queued(app, &ctx);
            let fraction = crate::notices::progress(app, &ctx, crate::notices::ProgressKey::Print)?;
            let id = app.progress_cards.get(&crate::notices::ProgressKey::Print)?;
            let cancellable = crate::notices::posted(&ctx).iter().any(|c| c.id == *id && !c.toast.actions.is_empty());
            Some((fraction, cancellable))
        };
        // The outcome notices: every card but a job's progress card, oldest first.
        let outcomes = |app: &mut PdfCraftApp| {
            crate::notices::post_queued(app, &ctx);
            crate::notices::posted(&ctx)
                .into_iter()
                .filter(|c| c.toast.progress.is_none())
                .map(|c| format!("{} {}", c.toast.title.as_ref().map_or("", |t| t.as_str()), c.toast.message.as_str()))
                .collect::<Vec<_>>()
        };

        let count = Arc::new(crate::JobCount::default());
        app.print_tasks.running.push(RunningPrint { ticket: 0, printer: "Office".into(), count: count.clone() });
        app.show_print_progress("Office", None);
        let (started, cancellable) = card(&mut app).expect("a card shows while the job runs");
        assert!(!cancellable);

        count.set(1, 3);
        app.poll_print();
        let first = card(&mut app).unwrap().0;
        assert!(first > started, "one sheet sent moves the card: {first}");

        count.set(2, 3);
        app.poll_print();
        let second = card(&mut app).unwrap().0;
        assert!(second > first && second < 1.0, "the card keeps rising with the sheets: {second}");

        let before = outcomes(&mut app).len();
        count.set(3, 3);
        app.print_tasks.jobs.submit(0, Priority::VISIBLE, |_| Ok(String::new()));
        jobs.run_all();
        app.poll_print();
        assert!(card(&mut app).is_none() && app.print_tasks.running.is_empty());
        let posted = outcomes(&mut app);
        assert_eq!(posted.len(), before + 1, "one outcome notice");
        assert!(posted.last().is_some_and(|m| m.contains("Office")), "it names the printer: {posted:?}");

        // A job the spooler refuses ends its card too, and says why.
        app.print_tasks.running.push(RunningPrint { ticket: 1, printer: "Office".into(), count: Arc::new(crate::JobCount::default()) });
        app.print_tasks.jobs.submit(1, Priority::VISIBLE, |_| Err("the spooler refused the job".into()));
        jobs.run_all();
        app.poll_print();
        assert!(card(&mut app).is_none() && app.print_tasks.running.is_empty());
        let posted = outcomes(&mut app);
        assert_eq!(posted.len(), before + 2, "the refusal is one more notice");
        assert!(posted.last().is_some_and(|m| m.contains("refused")), "{posted:?}");
    }

    /// Issue #1308: Current page is the page on screen, even when a later page is labelled with
    /// its number. A typed range still reads labels first.
    #[test]
    fn current_page_is_the_viewed_page_not_a_label() {
        let labels: Vec<String> = ["i", "ii", "iii", "1", "2"].map(String::from).to_vec();
        for (current, want) in [(0, vec![0]), (1, vec![1]), (3, vec![3])] {
            let d = PrintDraft { which: Which::Current, current_page: current, ..PrintDraft::default() };
            assert_eq!(d.settings(5, &labels).map(|s| s.pages), Ok(want), "viewing page {}", current + 1);
        }
        let typed = PrintDraft { which: Which::Range, range: "1".into(), ..PrintDraft::default() };
        assert_eq!(typed.settings(5, &labels).map(|s| s.pages), Ok(vec![3]));
    }

    #[test]
    fn print_preview_rasters_stay_within_the_texture_limit() {
        // Actual size on a 14,400 pt (200 in) page would ask for a raster far past any texture.
        let d = PrintDraft { size: SizeMode::Actual, ..PrintDraft::default() };
        let sizes = vec![(14_400.0, 14_400.0); 2];
        let labels = vec!["1".into(), "2".into()];
        for max_side in [2048.0, 8192.0, 16384.0, f32::NAN, 0.0] {
            for (page, scale) in preview_rasters(&d, &sizes, &labels, 4.0, max_side, PANE) {
                let long = 14_400.0 * scale;
                let cap = if max_side.is_finite() && max_side >= 1.0 { max_side.min(PREVIEW_MAX_SIDE) } else { PREVIEW_MAX_SIDE };
                assert!(long <= cap + 1.0, "page {page} at {max_side}: {long} px");
            }
        }
    }

    #[test]
    fn print_preview_rasters_keep_the_current_sheet_within_the_byte_budget() {
        // Many large pages: the current sheet always gets its raster; neighbours only while the
        // budget lasts, and the total stays within it.
        let d = PrintDraft { sheet: 5, ..PrintDraft::default() };
        let sizes = vec![(2_000.0, 2_000.0); 12];
        let labels: Vec<String> = (1..=12).map(|n| n.to_string()).collect();
        let rasters = preview_rasters(&d, &sizes, &labels, 4.0, 16384.0, PANE);
        assert_eq!(rasters.first().map(|r| r.0), Some(5), "the sheet on screen comes first: {rasters:?}");
        let total: f64 = rasters.iter().map(|(_, s)| (2_000.0 * f64::from(*s)).ceil().powi(2) * 4.0).sum();
        assert!(total <= PREVIEW_BYTES || rasters.len() == 1, "{total} bytes for {rasters:?}");
    }

    #[test]
    fn print_preview_is_sharper_than_a_thumbnail_on_a_high_dpi_screen() {
        let d = PrintDraft::default();
        let sizes = vec![(612.0, 792.0); 3];
        let labels = vec!["1".into(), "2".into(), "3".into()];
        assert!(preview_rasters(&d, &sizes, &labels, 2.0, 8192.0, None).is_empty(), "no drawn pane, no raster");
        let rasters = preview_rasters(&d, &sizes, &labels, 2.0, 8192.0, PANE);
        let scale = rasters.iter().find(|(page, _)| *page == 0).map(|(_, s)| *s).unwrap();
        // A 132 pt thumbnail at 2× is about 0.43 px/pt. The preview pane needs roughly twice that.
        assert!(scale > 0.7, "letter page in the preview at 2 px/pt: {scale}");
        assert!(rasters.iter().any(|(page, _)| *page == 1), "the next sheet is ready");
        let mut later = d.clone();
        later.sheet = 2;
        let rasters = preview_rasters(&later, &sizes, &labels, 2.0, 8192.0, PANE);
        assert!(rasters.iter().any(|(page, _)| *page == 2));
    }
}
