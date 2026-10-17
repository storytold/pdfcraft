//! Network printing (WS-Print) on craft-print's `wsd` backend. Cargo feature `network-print`; not
//! built for wasm32.
//!
//! The protocol, the print ticket and the job classification are craft-print's; this module is the
//! part PdfCraft adds on top, shared by the tools, the command line and the Print dialog:
//!
//! * [`Printer`]: one printer the person chose, behind a private [`craft_print::Registry`] that holds
//!   a [`wsd::WsdBackend`] with only that printer added. It asks what the printer is
//!   ([`Printer::info`]), sends a document ([`Printer::submit`], through
//!   [`Registry::submit_job`] with [`JobOptions`]) and follows the accepted job
//!   ([`Printer::watch`]) with craft-print's [`JobWatch`].
//! * **Two phases.** [`Printer::submit`] returns when the printer has *accepted* the document
//!   ([`Sent`]); it never waits for the paper. Following the job is a separate, caller-driven step,
//!   so a UI can tick it from a worker and a command line can run it to the end.
//! * **No retry of an unknown outcome.** Nothing here sends a document twice. A failure whose
//!   [`PrintError::outcome_unknown`] is true (the connection closed after the document was written)
//!   may have created a job: the caller tells the person to check the printer, and
//!   [`Printer::reconcile`] reads the printer's job lists (reads only) to find out.
//!
//! Per-job settings are the registry's: copies and sides in a [`Job`]; paper, orientation, quality,
//! resolution, an explicit colour choice and the page count in [`JobOptions`]. What the backend
//! keeps for itself (strict mode, the ticket shape, the transfer encoding) is [`Settings`], fixed
//! when a [`Printer`] is opened.
//!
//! **Formats and conversion are the registry's.** A [`Document`] in any format the printer lists
//! (PDF, PWG raster, PostScript, JPEG, PNG, TIFF, text, XPS, Open XPS) is sent as it is. One the
//! printer does not list goes through the app's [`DocumentConverter`], registered once at start-up
//! with [`register_pdf_converter`] ([`PdfToRaster`]: a PDF becomes PWG raster, rendered with
//! PdfCraft's own renderer at the resolution the printer lists nearest 300 dpi); with no way to get
//! there the print is [`ErrorKind::UnsupportedFormat`] before a job exists. The printer's details
//! (what `GetPrinterElements` returned) are handed to the registry with
//! [`Registry::submit_job_with_details`], so the registry does not read them a second time.
//! Every job needs a [`UserConfirmed`], which the type of [`Printer::submit`] requires.

use std::sync::{Arc, Mutex, PoisonError, RwLock};

pub use craft_print::wsd;
use craft_print::wsd::error::Op;
use craft_print::wsd::{Client, PrinterElements, PrinterUrl, Reconciled, TicketShape, Transfer, WsdBackend};
pub use craft_print::{
    CancelToken, ColourMode, Conversion, ConversionPolicy, ConvertContext, Document, DocumentConverter, DocumentFormat, Duplex, ErrorKind, Job,
    JobId, JobOptions, JobSource, JobState, JobStatus, JobWatch, Orientation, OwnedDocument, Phase, PrintError, PrinterDetails, Progress, Quality,
    Registry, Resolution, RetrySafety, Submitted, UserConfirmed, WaitPolicy, WatchOutcome, WatchStep, pwg,
};
use craft_print::{SubmitRequest, namespaced_id};

mod pdf;
pub use pdf::PdfToRaster;

/// A transport shared with worker threads: the plain-HTTP one of craft-wsd, or an in-memory fake.
pub type Transport = Arc<dyn wsd::adapter::Transport + Send + Sync>;

/// The registry name of the WS-Print backend (the prefix of its job ids).
const BACKEND: &str = "wsd";

/// The converter every [`Printer`] opened afterwards registers on its registry.
static CONVERTER: RwLock<Option<Arc<dyn DocumentConverter>>> = RwLock::new(None);

/// Register the converter the app brings: printers opened from now on use it for a document the
/// printer does not list. Replaces an earlier one. Done once, where network printing is set up.
pub fn register_converter(converter: Arc<dyn DocumentConverter>) {
    *CONVERTER.write().unwrap_or_else(PoisonError::into_inner) = Some(converter);
}

/// Register [`PdfToRaster`] (PDF to PWG raster with PdfCraft's renderer) unless a converter is
/// already registered. Safe to call from every start-up path.
pub fn register_pdf_converter() {
    let mut slot = CONVERTER.write().unwrap_or_else(PoisonError::into_inner);
    if slot.is_none() {
        *slot = Some(Arc::new(PdfToRaster));
    }
}

fn registered_converter() -> Option<Arc<dyn DocumentConverter>> {
    CONVERTER.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// What the backend keeps for itself, whatever the job: set when a printer is opened. The default
/// is craft-print's (the full ticket, MTOM, not strict).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Refuse a setting the printer does not list, and send every setting as mandatory.
    pub strict: bool,
    /// How much of the print ticket to send (quirk modes for firmware that stalls on the full one).
    pub ticket_shape: TicketShape,
    /// With a reduced ticket, drop the settings it cannot carry (a warning each) instead of refusing.
    pub drop_ticket_settings: bool,
    pub transfer: Transfer,
}

impl Default for Settings {
    fn default() -> Self {
        Self { strict: false, ticket_shape: TicketShape::Full, drop_ticket_settings: false, transfer: Transfer::Mtom }
    }
}

/// What a printer accepted: its job and what was sent.
#[derive(Clone, Debug)]
pub struct Sent {
    /// The registry's own record of the acceptance (message, warnings, what it can do afterwards).
    pub submitted: Submitted,
    /// The accepted job (`wsd:<printer url>#<number>`).
    pub job: JobId,
    /// The job's number on the printer.
    pub job_number: u32,
    /// The format the document was sent in, as the printer spells it.
    pub format: String,
    /// What the registry converted, when the document was not in a format the printer lists.
    pub conversion: Option<Conversion>,
    pub bytes_sent: usize,
    pub printer_name: String,
    /// Settings that were dropped or changed because the printer does not list them.
    pub warnings: Vec<String>,
}

/// One network printer: checked address, transport and the registry that follows its jobs.
pub struct Printer {
    url: PrinterUrl,
    client: Client<Transport>,
    backend: WsdBackend,
    registry: Registry,
    known: Mutex<Option<Known>>,
}

/// What a read of the printer left: the details the registry takes, the formats as the printer
/// spells them and its name.
#[derive(Clone)]
struct Known {
    details: PrinterDetails,
    listed: Vec<String>,
    name: Option<String>,
}

impl Printer {
    /// A printer at `address` (`http://` and a numeric loopback, link-local or private IP address:
    /// craft-wsd's policy decides, and a refusal is [`ErrorKind::InvalidRequest`]). `transport`
    /// replaces the plain-HTTP one (tests, embedding). Opens no connection.
    pub fn open(address: &str, transport: Option<Transport>) -> Result<Self, PrintError> {
        Self::open_with(address, transport, Settings::default())
    }

    /// [`Self::open`] with the backend's own [`Settings`].
    pub fn open_with(address: &str, transport: Option<Transport>, settings: Settings) -> Result<Self, PrintError> {
        let transport = transport.unwrap_or_else(wsd::adapter::default_transport);
        let backend = WsdBackend::new()
            .with_transport(Arc::clone(&transport))
            .with_strict(settings.strict)
            .with_ticket_shape(settings.ticket_shape)
            .with_drop_ticket_settings(settings.drop_ticket_settings)
            .with_transfer(settings.transfer);
        let id = backend.add(address)?;
        let url = PrinterUrl::parse(&id).map_err(PrintError::from)?;
        let registry = Registry::new();
        registry.register(Box::new(backend.clone())).map_err(|e| PrintError::new(ErrorKind::Failed, e.to_string()))?;
        if let Some(converter) = registered_converter() {
            registry.set_converter(converter);
        }
        Ok(Self { client: Client::new(transport, url.clone()), url, backend, registry, known: Mutex::new(None) })
    }

    /// Use `converter` for this printer instead of the registered one (tests, embedding).
    pub fn set_converter(&self, converter: Arc<dyn DocumentConverter>) {
        self.registry.set_converter(converter);
    }

    /// The registry's choices around conversion (preferred target formats, the resolution to
    /// approach); craft-print's defaults otherwise.
    pub fn set_conversion_policy(&self, policy: ConversionPolicy) {
        self.registry.set_conversion_policy(policy);
    }

    /// The canonical address.
    pub fn address(&self) -> String {
        self.url.as_url()
    }

    /// What the printer is and what it accepts (`GetPrinterElements`: description, status and
    /// capabilities). Sends no job. The answer is kept: [`Self::submit`] uses it instead of reading
    /// the printer again.
    pub fn info(&self) -> Result<PrinterElements, PrintError> {
        let elements = wsd::printer_info(&self.client).map_err(|e| e.print_error(Op::Query))?;
        self.remember(&elements);
        Ok(elements)
    }

    /// Hand over the answer of an earlier read of this printer (a dialog that has just shown it),
    /// so that the next [`Self::submit`] reads nothing before the registry decides on the format.
    /// Stale elements can only make that choice worse; the backend still checks against the printer.
    pub fn remember(&self, elements: &PrinterElements) {
        *self.known.lock().unwrap_or_else(PoisonError::into_inner) = Some(known_from(&self.url.as_url(), elements));
    }

    fn remembered(&self) -> Option<Known> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Send `document` and return when the printer has accepted it.
    ///
    /// A document in a format the printer lists is sent as it is; any other goes through the
    /// registered [`DocumentConverter`] (see the module documentation), and one that cannot be
    /// made into a listed format is [`ErrorKind::UnsupportedFormat`] before a job exists. `job` and
    /// `options` go to the registry as they are. `progress` gets the protocol phases (capabilities
    /// read, job created, document accepted); `cancel` is polled between phases, and cancelling after
    /// the job was created cancels it on the printer. An error says whether sending again is safe
    /// ([`PrintError::retry_safety`]). Set [`JobOptions::expected_pages`] to the document's page count.
    ///
    /// The printer is read once (`GetPrinterElements`) before the registry decides, unless
    /// [`Self::info`] or [`Self::remember`] already did; the backend then reads its capabilities
    /// once more to plan the ticket.
    pub fn submit(
        &self,
        document: Document<'_>,
        job: Job,
        options: JobOptions,
        consent: UserConfirmed,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Sent, PrintError> {
        if cancel.is_cancelled() {
            return Err(PrintError::new(ErrorKind::Cancelled, "the print job was cancelled before it started"));
        }
        let known = match self.remembered() {
            Some(known) => known,
            None => {
                self.info()?;
                self.remembered().ok_or_else(|| PrintError::new(ErrorKind::Failed, "the printer's details were not kept"))?
            }
        };
        let (format, len) = (document.format, document.bytes.len());
        let job = Job { printer: Some(namespaced_id(BACKEND, &self.url.as_url())), ..job };
        let request =
            SubmitRequest::new(document, job).with_options(options).with_consent(consent).with_cancel(cancel.clone()).with_progress(progress);
        let submitted = self.registry.submit_job_with_details(request, Some(known.details.clone()))?;
        let job_id =
            submitted.job.clone().ok_or_else(|| PrintError::new(ErrorKind::Failed, "the printer accepted the document but gave no job handle"))?;
        let job_number = WsdBackend::parse_job_id(&job_id)?.job_number;
        let conversion = submitted.conversion.clone();
        let sent_as = conversion.as_ref().map_or(format, |c| c.to);
        // The printer's own spelling of the type, else the standard one.
        let spelled =
            known.listed.iter().find(|l| DocumentFormat::from_mime(l) == Some(sent_as)).cloned().unwrap_or_else(|| sent_as.mime().to_owned());
        Ok(Sent {
            job: job_id,
            job_number,
            format: spelled,
            bytes_sent: conversion.as_ref().map_or(len, |c| c.bytes),
            conversion,
            printer_name: known.name.unwrap_or_else(|| self.url.as_url()),
            warnings: submitted.warnings.clone(),
            submitted,
        })
    }

    /// A follower for an accepted job, with the backend's poll support and interval and the
    /// request's expected totals. Tick it with [`Self::source`].
    pub fn watch(&self, sent: &Sent, policy: WaitPolicy) -> Result<JobWatch, PrintError> {
        JobWatch::for_submitted(&sent.submitted, policy).ok_or_else(|| PrintError::new(ErrorKind::Failed, "the accepted job cannot be followed"))
    }

    /// Where a [`JobWatch`] of this printer asks for status and sends its cancel.
    pub fn source(&self) -> &dyn JobSource {
        &self.registry
    }

    /// One read of a job's status (`GetJobElements`).
    pub fn status(&self, job: &JobId) -> Result<JobStatus, PrintError> {
        self.registry.job_status(job)
    }

    /// Ask the printer to cancel a job ("too late" is success).
    pub fn cancel(&self, job: &JobId) -> Result<(), PrintError> {
        self.registry.cancel_job(job)
    }

    /// After an error with [`PrintError::outcome_unknown`]: look for a job titled `title` on the
    /// printer (reads only), before anything is sent again.
    pub fn reconcile(&self, title: &str) -> Result<Reconciled, PrintError> {
        self.backend.reconcile(&self.url.as_url(), title)
    }
}

/// The registry's view of a printer, from what `GetPrinterElements` returned: the formats craft-print
/// has a name for (in the printer's order), colour, paper, copies and the listed resolutions.
fn known_from(id: &str, elements: &PrinterElements) -> Known {
    let name = elements.description.as_ref().map(|d| d.name.clone()).filter(|n| !n.is_empty());
    let mut details = PrinterDetails {
        id: id.to_owned(),
        name: name.clone().unwrap_or_else(|| id.to_owned()),
        colour: elements.description.as_ref().and_then(|d| d.color_supported),
        ..PrinterDetails::default()
    };
    let mut listed = Vec::new();
    if let Some(caps) = &elements.capabilities {
        for spelled in &caps.formats {
            if let Some(format) = DocumentFormat::from_mime(spelled)
                && !details.formats.contains(&format)
            {
                details.formats.push(format);
                listed.push(spelled.clone());
            }
        }
        details.two_sided = if caps.sides.is_empty() { None } else { Some(caps.sides.iter().any(|s| s.starts_with("TwoSided"))) };
        details.paper.clone_from(&caps.media_sizes);
        details.max_copies = caps.copies.map(|(_, max)| max);
        details.resolutions = caps.resolutions.iter().map(|&(x, y)| Resolution::new(x, y)).collect();
    }
    Known { details, listed, name }
}

/// A stable lowercase name for how following a job ended, for tool results and the dialog.
pub fn outcome_name(outcome: &WatchOutcome) -> &'static str {
    match outcome {
        WatchOutcome::Terminal(status) => match status.state {
            // craft-print models "completed with warnings" as Completed plus warnings on the status.
            JobState::Completed if !status.warnings.is_empty() => "completed_with_warnings",
            JobState::Completed => "completed",
            JobState::CompletedWithErrors => "completed_with_errors",
            JobState::Cancelled => "canceled",
            JobState::Aborted => "aborted",
            _ => "unknown",
        },
        WatchOutcome::Forgotten { .. } => "forgotten",
        WatchOutcome::NoStatus => "accepted_unknown",
        WatchOutcome::TimedOut { .. } => "still_running",
        WatchOutcome::Failed(_) => "failed",
        _ => "unknown",
    }
}

/// What the person is told when a send failed and the printer may nevertheless have the document.
pub const CHECK_THE_PRINTER: &str = "the request may have reached the printer: check the printer before sending again";
