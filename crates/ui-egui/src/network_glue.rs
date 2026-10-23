//! Print ▸ Network printer (WS-Print): the app's part of `craft_ui::printing::network`. The model,
//! its rules and the widgets are craft-ui's; the printing is craft-print's WS-Print backend through
//! `pdfcraft_print::network`. This file supplies what a host supplies: the address check, the
//! worker that asks a printer what it lists, the worker that sends a job and follows it, the
//! mapping of craft-print's answers onto craft-ui's light types, and the wording.
//!
//! Built only with the `network-print` feature on a target with sockets (the `network_print` cfg).
//! All network work runs on its own threads; the UI thread only starts it and drains receivers.
//!
//! The document is the print-ready PDF. A printer that lists PDF gets it as it is; one that lists only PWG
//! raster gets the pages rendered by `PdfToRaster`, the converter registered once when the destination is
//! created (the registry converts, at the resolution the printer lists nearest 300 dpi). The printer
//! the person queried is remembered, so the job does not read it again before the registry decides.
//!
//! A job has two phases and so does its worker: it sends the document and reports **accepted**
//! as soon as the printer has it (`JobUpdate::Accepted`), then follows the job with craft-print's
//! `JobWatch`, reporting each new status (`JobUpdate::Status`) until it ends. A document is sent
//! once: a send whose outcome is unknown is shown with the request to check the printer before
//! sending again (`Failure::outcome_unknown`), and nothing retries it.

use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use craft_ui::capability::Capability;
use craft_ui::printing::network::{
    self as net, Caps, Failure, FailureKind, JobHandle, JobUpdate, Labels, Outcome, PrinterSummary, Progress, QueryPrinter, Report, Request, SendJob,
    Stage,
};
use pdfcraft_print::network::wsd::{self as wsd, PrinterElements};
use pdfcraft_print::network::{
    CancelToken, ColourMode, Document, Duplex, ErrorKind, Job, JobOptions, JobState, JobStatus, Orientation, Phase, PrintError, Printer, Quality,
    Resolution, UserConfirmed, WaitPolicy, WatchOutcome, WatchStep,
};

use crate::PdfCraftApp;

/// Any transport that can be shared with the worker threads (tests give an in-memory printer).
pub type Transport = pdfcraft_print::network::Transport;

/// The media types the dialog can produce: the print-ready PDF, or its sheets as PWG raster
/// (the registry converts through `PdfToRaster` when the printer lists no PDF).
pub(crate) const FORMATS: &[&str] = &["application/pdf", "image/pwg-raster"];

/// What the last query of each address read, so a job to that printer does not read it again.
type Queried = Arc<Mutex<Option<(String, PrinterElements)>>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
const MAX_JOB_NAME: usize = 100;
/// How long a job is followed after the printer accepted it; it carries on at the printer after that.
const FOLLOW_FOR: Duration = Duration::from_secs(300);

/// The destination's state and the closures its workers are made from.
pub(crate) struct NetTasks {
    transport: Option<Transport>,
    pub(crate) state: net::State,
    query: Option<QueryPrinter>,
    queried: Queried,
}

impl Default for NetTasks {
    /// Creating the destination is the start-up of network printing: the app's `DocumentConverter`
    /// (PDF to PWG raster, with this build's renderer) is registered once, here.
    fn default() -> Self {
        pdfcraft_print::network::register_pdf_converter();
        Self { transport: None, state: net::State::default(), query: None, queried: Queried::default() }
    }
}

impl NetTasks {
    pub(crate) fn set_transport(&mut self, transport: Transport) {
        self.transport = Some(transport);
        self.query = None;
    }

    pub(crate) fn transport(&self) -> Option<Transport> {
        self.transport.clone()
    }

    /// What the last query read of the printer at `address`, when it is the same printer.
    fn queried(&self, address: &str) -> Option<PrinterElements> {
        lock(&self.queried).as_ref().filter(|(a, _)| a == address).map(|(_, e)| e.clone())
    }

    /// Draw the section; `sheets_ok`: the dialog's settings give something to print.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, sheets_ok: bool) -> net::Response {
        let (transport, ctx, queried) = (self.transport.clone(), ui.ctx().clone(), Arc::clone(&self.queried));
        let query = self.query.get_or_insert_with(|| query_printer(transport, Some(ctx), queried));
        let host = net::Host { validate: &validate, query, sheets_ok, formats: FORMATS };
        self.state.show(ui, egui::Id::new("network-printer"), &host, &labels())
    }
}

/// The address checked against the network policy (a numeric loopback, link-local or private
/// address). The error is what the dialog shows under the field.
pub(crate) fn validate(text: &str) -> Result<(), Failure> {
    wsd::PrinterUrl::parse(text).map(|_| ()).map_err(|e| Failure::new(FailureKind::Policy, e.to_string()))
}

/// What went wrong, as craft-ui words it. `open`: the failure is the printer's address being refused.
fn failure(e: &PrintError, open: bool) -> Failure {
    let kind = if e.native().is_some_and(|n| n.domain == "wsd-fault") {
        FailureKind::Fault
    } else if e.phase() == Some(Phase::Prepare) && e.kind() == ErrorKind::Failed {
        // The document could not be made into something the printer takes (the converter failed).
        FailureKind::Encode
    } else {
        match e.kind() {
            ErrorKind::InvalidRequest if open => FailureKind::Policy,
            ErrorKind::InvalidRequest | ErrorKind::ConsentRequired => FailureKind::Invalid,
            ErrorKind::UnsupportedFormat => FailureKind::NoCommonFormat,
            ErrorKind::Unsupported => FailureKind::Unsupported,
            ErrorKind::Unavailable | ErrorKind::Timeout | ErrorKind::Busy | ErrorKind::Io => FailureKind::Transport,
            ErrorKind::Failed => FailureKind::Protocol,
            ErrorKind::Rejected | ErrorKind::NotFound => FailureKind::Fault,
            ErrorKind::Cancelled => FailureKind::Stopped,
            _ => FailureKind::Other,
        }
    };
    Failure::new(kind, e.message()).with_outcome_unknown(e.outcome_unknown())
}

fn summary(elements: PrinterElements) -> PrinterSummary {
    let caps = elements.capabilities.unwrap_or_default();
    let description = elements.description.unwrap_or_default();
    let status = elements.status.unwrap_or_default();
    PrinterSummary {
        name: description.name,
        color: description.color_supported,
        state: status.state,
        queued_jobs: status.queued_jobs,
        reasons: status.reasons,
        caps: caps_of(&caps, description.color_supported),
    }
}

/// What the printer published, as craft-ui's three-state `Caps`. WS-Print cannot say "an empty
/// list": a list the printer left out is empty here, which reads as unknown (not unsupported), so the
/// control stays enabled with a note and the value is sent as a hint. WS-Print publishes no colour
/// list, only whether the printer prints in colour.
fn caps_of(caps: &wsd::Capabilities, color: Option<bool>) -> Caps {
    let tokens = |list: &[String]| if list.is_empty() { Capability::Unknown } else { Capability::from_option_list(Some(list)) };
    let resolutions: Vec<net::Resolution> = caps.resolutions.iter().map(|&(x, y)| net::Resolution::new(x, y)).collect();
    Caps {
        formats: caps.formats.clone(),
        sides: tokens(&caps.sides),
        orientations: tokens(&caps.orientations),
        qualities: tokens(&caps.qualities),
        color_modes: match color {
            None => Capability::Unknown,
            Some(true) => {
                Capability::from_option_list(Some(&[net::ColorMode::Color.token().to_owned(), net::ColorMode::Monochrome.token().to_owned()]))
            }
            Some(false) => Capability::from_option_list(Some(&[net::ColorMode::Monochrome.token().to_owned()])),
        },
        media_sizes: tokens(&caps.media_sizes),
        resolutions: if resolutions.is_empty() { Capability::Unknown } else { Capability::from_option_list(Some(&resolutions)) },
        copies: Capability::from_option_range(caps.copies),
    }
}

fn repaint(ctx: &Option<egui::Context>) {
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
}

/// The worker that asks the printer at the (normalised) address what it is and what it lists. It
/// sends no job.
pub(crate) fn query_printer(transport: Option<Transport>, ctx: Option<egui::Context>, queried: Queried) -> QueryPrinter {
    Box::new(move |address: &str| {
        let (tx, rx) = mpsc::channel();
        let (address, transport, ctx, queried) = (address.to_owned(), transport.clone(), ctx.clone(), Arc::clone(&queried));
        std::thread::spawn(move || {
            let result = match Printer::open(&address, transport) {
                Ok(printer) => printer
                    .info()
                    .map(|elements| {
                        *lock(&queried) = Some((printer.address(), elements.clone()));
                        summary(elements)
                    })
                    .map_err(|e| failure(&e, false)),
                Err(e) => Err(failure(&e, true)),
            };
            let _ = tx.send(result);
            repaint(&ctx);
        });
        rx
    })
}

/// The `MediaSizeName` for a sheet size name of the Print dialog, for the sizes printers list.
pub(crate) fn media_for_paper(paper: &str) -> Option<&'static str> {
    Some(match paper {
        "US Letter" => "na_letter_8.5x11in",
        "US Legal" => "na_legal_8.5x14in",
        "Tabloid" => "na_ledger_11x17in",
        "A3" => "iso_a3_297x420mm",
        "A4" => "iso_a4_210x297mm",
        "A5" => "iso_a5_148x210mm",
        _ => return None,
    })
}

/// The job and its per-job options for the registry. `Photo` is craft-print's `Quality::Photo`: the
/// backend maps it to the printer's own quality when the printer lists one and says so when it does not.
fn options(request: &Request, job_name: &str) -> (Job, JobOptions) {
    let name: String = job_name.chars().filter(|c| !c.is_control()).take(MAX_JOB_NAME).collect();
    let mut job = Job::default();
    if !name.trim().is_empty() {
        job.title = name;
    }
    job.copies = request.copies.unwrap_or(1);
    job.duplex = request.sides.map(|s| match s {
        net::Sides::OneSided => Duplex::Off,
        net::Sides::LongEdge => Duplex::LongEdge,
        net::Sides::ShortEdge => Duplex::ShortEdge,
    });
    let mut options = JobOptions::new();
    if let Some(o) = request.orientation {
        options = options.with_orientation(match o {
            net::Orientation::Portrait => Orientation::Portrait,
            net::Orientation::Landscape => Orientation::Landscape,
            net::Orientation::ReversePortrait => Orientation::ReversePortrait,
            net::Orientation::ReverseLandscape => Orientation::ReverseLandscape,
        });
    }
    match request.quality {
        Some(net::Quality::Draft) => options = options.with_quality(Quality::Draft),
        Some(net::Quality::Normal) => options = options.with_quality(Quality::Normal),
        Some(net::Quality::High) => options = options.with_quality(Quality::High),
        Some(net::Quality::Photo) => options = options.with_quality(Quality::Photo),
        None => {}
    }
    if let Some(color) = request.color {
        options = options.with_colour(match color {
            net::ColorMode::Color => ColourMode::Colour,
            net::ColorMode::Monochrome => ColourMode::Mono,
        });
    }
    if let Some(r) = request.resolution {
        options = options.with_resolution(Resolution::new(r.x_dpi, r.y_dpi));
    }
    if let Some(media) = &request.media_size {
        options = options.with_media(media.as_str());
    }
    if let Some(pages) = request.expected_pages {
        options = options.with_expected_pages(pages);
    }
    (job, options)
}

/// What the printer says about an accepted job, as craft-ui shows it.
fn progress(status: &JobStatus) -> Progress {
    Progress {
        stage: Stage::from_name(status.state.name()),
        sheets_done: status.sheets_done,
        sheets_total: status.sheets_total,
        notes: status.reasons.clone(),
    }
}

/// How following a job ended, as craft-ui's report. The document was accepted in every case, so
/// a watch that could not finish is "sent, outcome unknown", never a failure.
fn report(outcome: &WatchOutcome, printer: &str, warnings: &[String]) -> Report {
    let (kind, status) = match outcome {
        WatchOutcome::Terminal(s) => (
            match s.state {
                // craft-print models "completed with warnings" as Completed plus warnings on the status.
                JobState::Completed if !s.warnings.is_empty() => Outcome::CompletedWithWarnings,
                JobState::Completed => Outcome::Completed,
                JobState::CompletedWithErrors => Outcome::CompletedWithErrors,
                JobState::Cancelled => Outcome::Canceled,
                JobState::Aborted => Outcome::Aborted,
                _ => Outcome::AcceptedUnknown,
            },
            Some(s),
        ),
        WatchOutcome::Forgotten { last } => (Outcome::Forgotten, last.as_ref()),
        WatchOutcome::TimedOut { last } => (Outcome::StillRunning, last.as_ref()),
        _ => (Outcome::AcceptedUnknown, None),
    };
    let mut notes: Vec<String> = status.map(|s| s.reasons.clone()).unwrap_or_default();
    notes.extend(warnings.iter().cloned());
    if let WatchOutcome::Failed(e) = outcome {
        notes.push(e.message().to_owned());
    }
    Report { outcome: kind, printer: printer.to_owned(), sheets: status.and_then(|s| s.sheets_done).map_or(0, u64::from), notes }
}

/// The worker that sends the print-ready `pdf` and follows the job. `queried` is what the last query read of
/// the printer, when it is the one addressed. Each
/// call makes a new job, with the person's `confirmed` (never `interactive()`: it is the attestation
/// of the ticked box, taken when Print was pressed).
pub(crate) fn send_job(
    transport: Option<Transport>,
    ctx: Option<egui::Context>,
    name: String,
    pdf: Arc<Vec<u8>>,
    queried: Option<PrinterElements>,
    confirmed: UserConfirmed,
) -> SendJob {
    Box::new(move |address: &str, request: Request| {
        let (result_tx, result_rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        let cancel = CancelToken::new();
        let (address, transport, ctx) = (address.to_owned(), transport.clone(), ctx.clone());
        let (name, pdf, queried, worker_cancel) = (name.clone(), Arc::clone(&pdf), queried.clone(), cancel.clone());
        std::thread::spawn(move || {
            let (job, options) = options(&request, &name);
            let result = run_job(&address, transport, (job, options), &pdf, queried.as_ref(), confirmed, &worker_cancel, &update_tx, &ctx);
            let _ = result_tx.send(result);
            repaint(&ctx);
        });
        JobHandle::new(result_rx, Box::new(move || cancel.cancel())).with_updates(update_rx)
    })
}

#[allow(clippy::too_many_arguments)]
fn run_job(
    address: &str,
    transport: Option<Transport>,
    (job, options): (Job, JobOptions),
    pdf: &Arc<Vec<u8>>,
    queried: Option<&PrinterElements>,
    confirmed: UserConfirmed,
    cancel: &CancelToken,
    updates: &mpsc::Sender<JobUpdate>,
    ctx: &Option<egui::Context>,
) -> Result<Report, Failure> {
    let printer = Printer::open(address, transport).map_err(|e| failure(&e, true))?;
    // What the person was shown is what the registry decides on: no second read of the printer.
    if let Some(elements) = queried {
        printer.remember(elements);
    }
    // Phase one: the printer accepts the document. A cancel before that sends nothing, or cancels the job just made.
    let sent = match printer.submit(Document::pdf(pdf.as_slice()), job, options, confirmed, cancel, &mut |_| {}) {
        Ok(sent) => sent,
        Err(e) if e.kind() == ErrorKind::Cancelled => {
            return Ok(Report { outcome: Outcome::Canceled, printer: String::new(), sheets: 0, notes: Vec::new() });
        }
        Err(e) => return Err(failure(&e, false)),
    };
    let notes = sent.warnings.clone();
    let _ = updates.send(JobUpdate::Accepted(Progress { notes: notes.clone(), ..Progress::default() }));
    repaint(ctx);
    // Phase two: follow it. One cancel is sent if asked; the watch then carries on to the end state.
    let policy = WaitPolicy { interval: Some(Duration::from_millis(500)), ..WaitPolicy::default() }.with_max_wait(FOLLOW_FOR);
    let outcome = match printer.watch(&sent, policy) {
        Ok(mut watch) => {
            watch = watch.with_cancel(cancel.clone());
            loop {
                match watch.tick(printer.source()) {
                    WatchStep::Pending { next_in } => std::thread::sleep(next_in.min(Duration::from_millis(100))),
                    WatchStep::Update(status) => {
                        let _ = updates.send(JobUpdate::Status(progress(&status)));
                        repaint(ctx);
                    }
                    WatchStep::Done(outcome) => break outcome,
                    _ => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        }
        Err(e) => WatchOutcome::Failed(e),
    };
    Ok(report(&outcome, &sent.printer_name, &notes))
}

/// The wording of the destination, in the interface language. Templates keep their `{...}` holes.
pub(crate) fn labels() -> Labels<'static> {
    Labels {
        address: tl!("Printer address:"),
        address_hint: "http://192.168.1.50:5357/service",
        address_note: tl!("Only IP addresses on this network are accepted (private, link-local or loopback), not host names."),
        query_printer: tl!("Query printer"),
        querying: tl!("Asking the printer…"),
        query_first: tl!("Query the printer first to see what it supports."),
        printer_status: tl!("Printer status: {state}"),
        reasons: tl!("Reasons: {reasons}"),
        no_common_format: tl!("The printer accepts none of the formats this document can be sent in"),
        copies: tl!("Copies:"),
        sides: tl!("Two-sided:"),
        sides_off: tl!("Off"),
        sides_long_edge: tl!("Flip on long edge"),
        sides_short_edge: tl!("Flip on short edge"),
        orientation: tl!("Printer orientation:"),
        portrait: tl!("Portrait"),
        landscape: tl!("Landscape"),
        reverse_portrait: tl!("Reverse portrait"),
        reverse_landscape: tl!("Reverse landscape"),
        quality: tl!("Quality:"),
        draft: tl!("Draft"),
        normal: tl!("Normal"),
        high: tl!("High"),
        photo: tl!("Photo"),
        color: tl!("Color:"),
        color_color: tl!("Colour"),
        color_monochrome: tl!("Black and white"),
        resolution: tl!("Resolution:"),
        resolution_dpi: "{dpi} dpi",
        resolution_dpi_xy: "{x} x {y} dpi",
        auto: tl!("Auto"),
        unsupported_copies: tl!("This printer does not take a copies setting."),
        unsupported_sides: tl!("This printer only prints one-sided."),
        unsupported_orientation: tl!("This printer does not take an orientation setting."),
        unsupported_quality: tl!("This printer does not take a quality setting."),
        unsupported_color: tl!("This printer only prints in black and white."),
        unsupported_resolution: tl!("This printer does not take a resolution setting."),
        unconfirmed_note: tl!("Not confirmed by the printer"),
        confirm: tl!("I confirm sending this document to this printer"),
        sending: tl!("Sending to {printer}…"),
        canceling: tl!("Canceling…"),
        cancel: tl!("Cancel"),
        sent: tl!("Sent to {printer}"),
        accepted: tl!("Accepted by {printer}"),
        stage_waiting: tl!("Waiting in the printer's queue"),
        stage_held: tl!("Held by the printer"),
        stage_printing: tl!("The printer is printing it"),
        stage_stopped: tl!("The printer paused the job; it needs attention"),
        sheets: tl!("Sheets printed: {n}"),
        sheets_progress: tl!("Sheets printed: {done} of {total}"),
        outcome_warnings: tl!("Printed with warnings"),
        outcome_errors: tl!("Printed with errors"),
        outcome_canceled: tl!("The job was canceled"),
        outcome_aborted: tl!("The printer stopped the job"),
        outcome_forgotten: tl!("The printer no longer lists the job; it most likely finished"),
        outcome_still_running: tl!("Still printing; the job continues on the printer"),
        outcome_accepted_unknown: tl!("Sent. The printer can't say whether it printed."),
        outcome_unknown_warning: tl!("The printer may have received the document. Check the printer before sending again."),
        failure_policy: tl!("The printer address was refused"),
        failure_transport: tl!("Couldn't reach the printer"),
        failure_protocol: tl!("The printer's reply couldn't be understood"),
        failure_fault: tl!("The printer refused the request"),
        failure_no_common_format: tl!("The printer accepts none of the formats this document can be sent in"),
        failure_unsupported: tl!("The printer doesn't support this setting"),
        failure_encode: tl!("The document couldn't be prepared"),
        failure_invalid: tl!("A setting isn't valid"),
        failure_stopped: tl!("The print job stopped unexpectedly"),
        failure_other: tl!("The print job stopped unexpectedly"),
    }
}

impl PdfCraftApp {
    /// Use `transport` for network printing instead of the built-in HTTP one (tests; embedding).
    pub fn set_wsd_transport(&mut self, transport: Transport) {
        self.print_tasks.net.set_transport(transport);
    }

    /// Send the print-ready `pdf` (laid out as `sheets`, whose count is the job's page count) to the
    /// queried printer, off the UI thread. It needs the person's confirmation, which it spends.
    /// Returns `true` when a job started.
    pub(crate) fn network_send(&mut self, name: &str, pdf: Vec<u8>, sheets: Vec<(f64, f64)>, paper: &str) -> bool {
        let page_count = sheets.len();
        let net = &mut self.print_tasks.net;
        let address = net.state.draft.normalized_url();
        // The attestation comes from the ticked box and nothing else.
        let Ok(confirmed) = UserConfirmed::attest(Some(net.state.draft.confirmed)) else { return false };
        let queried = pdfcraft_print::network::wsd::PrinterUrl::parse(&address).ok().and_then(|u| net.queried(&u.as_url()));
        let send = send_job(net.transport(), self.ctx.clone(), name.to_owned(), Arc::new(pdf), queried, confirmed);
        let net = &mut self.print_tasks.net;
        // The sheets are the pages of the document sent: the total that progress shows until the printer reports one.
        net.state.draft.expected_pages = u32::try_from(page_count).ok();
        net.state.start_job(&send, validate(&address).is_ok(), true, FORMATS, media_for_paper(paper))
    }

    /// Collect finished work (each frame), also while the dialog is closed; with the dialog gone the
    /// person still hears how a job went.
    pub(crate) fn poll_network(&mut self) {
        let was_running = self.print_tasks.net.state.draft.running();
        let ctx = self.ctx.clone().unwrap_or_default();
        self.print_tasks.net.state.poll(&ctx);
        if !was_running || self.print_tasks.net.state.draft.running() || self.dialog == Some(crate::Dialog::Print) {
            return;
        }
        let labels = labels();
        let message = match &self.print_tasks.net.state.draft.job {
            net::JobView::Finished(r) => {
                let text = labels.outcome_text(r.outcome).map_or_else(|| crate::i18n::fmt(labels.sent, &[("printer", &r.printer)]), str::to_owned);
                Some(text)
            }
            net::JobView::Failed(f) => {
                let mut text = format!("{}: {}", labels.failure_heading(f.kind), f.detail);
                if f.outcome_unknown {
                    text = format!("{text} {}", labels.outcome_unknown_warning);
                }
                Some(text)
            }
            _ => None,
        };
        if let Some(message) = message {
            self.notify(message);
        }
    }
}

#[cfg(test)]
mod tests;
