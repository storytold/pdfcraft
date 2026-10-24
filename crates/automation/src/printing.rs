//! Printing tools: list printers, and print (or save the print-ready PDF) with Acrobat's Print
//! dialog options.

use pdfcraft_engine::print::{self, Binding, BookletSubset, Content, Layout, Orientation, PAPERS, PageOrder, SizeMode, Subset, spool};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed, write_atomic};

#[cfg(target_os = "linux")]
type PortalResult = std::result::Result<(print::portal::Outcome, usize), String>;
#[cfg(target_os = "linux")]
type PortalSubmit =
    std::sync::Arc<dyn Fn(&[u8], &str, &print::portal::Cancellation) -> std::result::Result<print::portal::Outcome, String> + Send + Sync>;

/// One active portal request, with bounded receipts for the latest sixteen jobs.
#[cfg(target_os = "linux")]
pub(crate) struct PortalJobs {
    next: u64,
    asynchronous: bool,
    jobs: std::collections::VecDeque<PortalJob>,
    submit: PortalSubmit,
}

#[cfg(target_os = "linux")]
impl Default for PortalJobs {
    /// Start without jobs; only MCP opts into asynchronous portal receipts.
    fn default() -> Self {
        Self {
            next: 0,
            asynchronous: false,
            jobs: Default::default(),
            submit: std::sync::Arc::new(|bytes, title, cancel| print::portal::submit(bytes, title, "", cancel).map_err(|e| e.to_string())),
        }
    }
}

/// Only small status receipts remain after the worker drops its PDF and descriptor.
#[cfg(target_os = "linux")]
struct PortalJob {
    id: u64,
    output: Value,
    cancel: print::portal::Cancellation,
    receiver: Option<std::sync::mpsc::Receiver<PortalResult>>,
}

#[cfg(target_os = "linux")]
impl Drop for PortalJob {
    /// Closing the owning automation session cancels a still-pending host request.
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(target_os = "linux")]
impl PortalJob {
    /// Poll without blocking MCP dispatch, and never mistake a lost worker for acceptance.
    fn refresh(&mut self) {
        let result = self.receiver.as_ref().and_then(|receiver| match receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("The system print worker stopped without a result".into())),
        });
        if let Some(result) = result {
            self.receiver = None;
            // output is always the object constructed by PortalJobs::start.
            match result {
                Ok((outcome, bytes)) => {
                    self.output["status"] = json!(match outcome {
                        print::portal::Outcome::Accepted => "accepted",
                        print::portal::Outcome::Cancelled => "cancelled",
                    });
                    self.output["bytes"] = json!(bytes);
                }
                Err(error) => {
                    self.output["status"] = json!("failed");
                    self.output["error"] = json!(error.chars().take(1024).collect::<String>());
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl PortalJobs {
    /// Refuse overlapping system dialogs before doing any layout or PDF composition.
    fn available(&mut self) -> Result<()> {
        for job in &mut self.jobs {
            job.refresh();
        }
        if let Some(job) = self.jobs.iter().find(|job| job.receiver.is_some()) {
            return Err(bad(format!("System print job {} is pending; inspect or cancel it with doc_print_job before printing again", job.id)));
        }
        Ok(())
    }

    /// Snapshot preparation and the host dialog run on a worker; dispatch returns a job id.
    fn start(&mut self, task: pdfcraft_engine::PrintTask, title: String, sheets: usize, pages: usize) -> Result<Value> {
        self.available()?;
        let id = self.next.checked_add(1).filter(|id| *id <= i64::MAX as u64).ok_or_else(|| failed("System print job ids exhausted"))?;
        let cancel = print::portal::Cancellation::default();
        let worker_cancel = cancel.clone();
        let submit = self.submit.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("automation-print-portal".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if worker_cancel.is_cancelled() {
                        return Ok((print::portal::Outcome::Cancelled, 0));
                    }
                    let bytes = task().map_err(|e| e.to_string())?;
                    let length = bytes.len();
                    let outcome =
                        if worker_cancel.is_cancelled() { print::portal::Outcome::Cancelled } else { submit(&bytes, &title, &worker_cancel)? };
                    Ok((outcome, length))
                }))
                .unwrap_or_else(|_| Err("Could not prepare the document for system printing".into()));
                let _ = sender.send(result);
            })
            .map_err(|e| failed(format!("Could not start system printing: {e}")))?;
        self.next = id;
        while self.jobs.len() >= 16 {
            self.jobs.pop_front();
        }
        let output = json!({ "job": id, "status": "pending", "sheets": sheets, "pages": pages });
        self.jobs.push_back(PortalJob { id, output: output.clone(), cancel, receiver: Some(receiver) });
        Ok(output)
    }

    /// Cancellation is a request; a completed or already accepted job keeps its true outcome.
    fn inspect(&mut self, id: u64, cancel: bool) -> Result<Value> {
        let job =
            self.jobs.iter_mut().find(|job| job.id == id).ok_or_else(|| bad("Unknown system print job; the latest sixteen receipts are retained"))?;
        job.refresh();
        if cancel && job.receiver.is_some() {
            job.cancel.cancel();
            job.output["cancellation_requested"] = json!(true);
        }
        Ok(job.output.clone())
    }
}

/// Reject unsupported portal arguments before page selection, layout, or PDF composition.
fn system_dialog_requested(a: &Args) -> Result<bool> {
    let (path, printer) = (a.opt_str("path")?, a.opt_str("printer")?);
    if !matches!((path, printer), (Some(_), None) | (None, Some(_))) {
        return Err(bad("pass either path (save the print-ready PDF) or printer (a name from printers, or \"default\")"));
    }
    let portal = spool::backend() == spool::Backend::Portal && path.is_none();
    if portal {
        let options = match a.get("options") {
            None => false,
            Some(Value::Object(options)) => !options.is_empty(),
            Some(_) => return Err(bad("options must be an object of option keys and choices from printer_options")),
        };
        if !matches!(printer, Some("default" | "system-dialog"))
            || a.get("copies").is_some()
            || a.get("duplex").is_some()
            || a.get("collate").is_some()
            || options
        {
            return Err(bad(
                "Flatpak printing uses the system dialog: use printer default or system-dialog; choose copies, collate, duplex and driver options there",
            ));
        }
    }
    Ok(portal)
}

/// The job's two-sided setting for `doc_print`'s `duplex`: `printer` sends none, so the printer's
/// own default applies (as the Print dialog's Printer default, #2422), except that cut and stack
/// needs single-sided sheets.
fn duplex(value: &str, cut_stack: bool) -> Result<Option<spool::Duplex>> {
    Ok(match value {
        "printer" if cut_stack => Some(spool::Duplex::Off),
        "printer" => None,
        "off" => Some(spool::Duplex::Off),
        "long-edge" if cut_stack => return Err(bad("cut-and-stack printing needs duplex off (single-sided sheets)")),
        "short-edge" if cut_stack => return Err(bad("cut-and-stack printing needs duplex off (single-sided sheets)")),
        "long-edge" => Some(spool::Duplex::LongEdge),
        "short-edge" => Some(spool::Duplex::ShortEdge),
        d => return Err(bad(format!("unknown duplex {d:?}"))),
    })
}

/// [`duplex`] for a job laid out as `layout`: the name is checked either way, and a booklet
/// decides two-sided itself (#2707), as the Print dialog does (`print::booklet_duplex`).
fn job_duplex(value: &str, layout: &Layout) -> Result<Option<spool::Duplex>> {
    let chosen = duplex(value, matches!(layout, Layout::Multiple { order: PageOrder::CutStack, .. }))?;
    Ok(match layout {
        Layout::Booklet { subset, .. } => Some(print::booklet_duplex(*subset)),
        _ => chosen,
    })
}

/// An invalid-arguments error saying `m`.
fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

/// `doc_print`'s `paper`, by its name or a short form ("Letter", "ARCH D"), as its `PAPERS`
/// index and size; US Letter when none is given.
fn paper_named(name: Option<&str>) -> Result<(usize, (f64, f64))> {
    let mut papers = PAPERS.iter().enumerate();
    let found = match name {
        None => papers.next(),
        Some(p) => papers.find(|(_, (n, _))| {
            n.eq_ignore_ascii_case(p)
                || n.replace("US ", "").eq_ignore_ascii_case(p)
                || n.split(" (").next().is_some_and(|name| name.eq_ignore_ascii_case(p))
        }),
    };
    found.map(|(index, (_, size))| (index, *size)).ok_or_else(|| {
        bad(format!(
            "unknown paper {:?}; choose one of {}",
            name.unwrap_or_default(),
            PAPERS.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ")
        ))
    })
}

/// The spooler job for `doc_print` to `printer`. It feeds the paper the PDF was laid out on
/// (`PAPERS[paper]`, as `media` after the driver's `options`) rather than the queue's default
/// paper, which a job naming none got (A4 under Letter sheets on the queue in #3752).
fn spool_job(a: &Args, printer: &str, title: String, duplex: Option<spool::Duplex>, paper: usize) -> Result<spool::Job> {
    let options = match a.get("options") {
        None => Vec::new(),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| match v.as_str() {
                Some(v) => Ok((k.clone(), v.to_string())),
                None => Err(bad(format!("options.{k} must be a string (a choice from printer_options)"))),
            })
            .collect::<Result<Vec<_>>>()?,
        Some(_) => return Err(bad("options must be an object of option keys and choices from printer_options")),
    };
    Ok(spool::Job {
        printer: if printer == "default" { None } else { Some(printer.to_string()) },
        copies: a.opt_int("copies")?.unwrap_or(1).clamp(1, 999) as u32,
        collate: a.opt_bool("collate")?.unwrap_or(true),
        duplex,
        scaling: spool::Scaling::Exact,
        grayscale: a.opt_bool("grayscale")?.unwrap_or(false),
        title,
        options: options.into_iter().chain(print::media_option(paper)).collect(),
    })
}

impl Automation {
    /// MCP must remain responsive while the host dialog is open; CLI/direct calls keep their
    /// existing synchronous receipts so a script cannot exit and cancel its own pending job.
    #[cfg(feature = "mcp")]
    pub(crate) fn nonblocking_system_printing(&mut self) {
        #[cfg(target_os = "linux")]
        {
            self.print_jobs.asynchronous = true;
        }
    }

    /// Inspect or request cancellation of a portal job without waiting for its worker.
    pub(crate) fn doc_print_job(&mut self, a: &Args) -> Result<Value> {
        let id = u64::try_from(a.int("job")?).map_err(|_| bad("job must be a positive system print job id"))?;
        let cancel = a.opt_bool("cancel")?.unwrap_or(false);
        #[cfg(target_os = "linux")]
        {
            self.print_jobs.inspect(id, cancel)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (id, cancel);
            Err(bad("System print jobs are available only for Linux portal printing"))
        }
    }

    /// `printers`: the system's printers, the default marked, and each one's two-sided support
    /// where the spooler reports it.
    pub(crate) fn printers(&self) -> Result<Value> {
        let list: Vec<Value> = spool::printers()
            .into_iter()
            .map(|p| {
                let mut v = json!({ "name": p.name, "default": p.default });
                if let Some(two_sided) = p.two_sided {
                    v["two_sided"] = json!(two_sided);
                }
                v
            })
            .collect();
        Ok(json!({ "count": list.len(), "printers": list, "system_dialog": spool::backend() == spool::Backend::Portal }))
    }

    pub(crate) fn printer_options(&self, a: &Args) -> Result<Value> {
        let printer = a.str("printer")?;
        let list: Vec<Value> = spool::printer_options(printer)
            .into_iter()
            .map(|o| {
                let choices: Vec<Value> = o.choices.iter().map(|(value, label)| json!({ "value": value, "label": label })).collect();
                json!({ "key": o.key, "label": o.label, "group": o.group, "default": o.default, "choices": choices })
            })
            .collect();
        Ok(json!({ "printer": printer, "count": list.len(), "options": list }))
    }

    /// `doc_print`: lay the document out with the Print dialog's options, then save the
    /// print-ready PDF to `path` or send it to `printer` with the chosen paper.
    /// MCP queues portal jobs; CLI/direct automation waits for the completed host receipt.
    pub(crate) fn doc_print(&mut self, a: &Args) -> Result<Value> {
        let portal = system_dialog_requested(a)?;
        #[cfg(target_os = "linux")]
        if portal {
            self.print_jobs.available()?;
        }
        let doc = self.doc(a)?;
        let (id, count) = (doc.id, doc.info.pages.len());
        let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        let subset = match a.opt_str("subset")?.unwrap_or("all") {
            "all" => Subset::All,
            "odd" => Subset::Odd,
            "even" => Subset::Even,
            s => return Err(bad(format!("unknown subset {s:?}"))),
        };
        let pages = print::select_pages(count, a.opt_str("pages")?, &labels, subset, a.opt_bool("reverse")?.unwrap_or(false))
            .map_err(|e| bad(e.to_string()))?;
        let layout = match a.opt_str("layout")?.unwrap_or("fit") {
            "fit" => Layout::Size(SizeMode::Fit),
            "actual" => Layout::Size(SizeMode::Actual),
            "shrink" => Layout::Size(SizeMode::Shrink),
            "custom" => Layout::Size(SizeMode::Custom(a.opt_num("scale")?.ok_or_else(|| bad("custom needs scale (percent)"))?)),
            "multiple" => {
                let per = a.opt_int("per_sheet")?.unwrap_or(2).clamp(1, 256) as usize;
                let order = match a.opt_str("order")?.unwrap_or("horizontal") {
                    "horizontal" => PageOrder::Horizontal,
                    "horizontal-reversed" => PageOrder::HorizontalReversed,
                    "vertical" => PageOrder::Vertical,
                    "vertical-reversed" => PageOrder::VerticalReversed,
                    "cut-stack" => PageOrder::CutStack,
                    o => return Err(bad(format!("unknown order {o:?}"))),
                };
                match Layout::multiple(per) {
                    Layout::Multiple { cols, rows, per_sheet, .. } => Layout::Multiple {
                        cols,
                        rows,
                        per_sheet,
                        order,
                        border: a.opt_bool("border")?.unwrap_or(false),
                        auto_rotate: a.opt_bool("auto_rotate")?.unwrap_or(true),
                    },
                    l => l,
                }
            }
            "booklet" => Layout::Booklet {
                subset: match a.opt_str("booklet_subset")?.unwrap_or("both") {
                    "both" => BookletSubset::BothSides,
                    "front" => BookletSubset::FrontOnly,
                    "back" => BookletSubset::BackOnly,
                    s => return Err(bad(format!("unknown booklet_subset {s:?}"))),
                },
                binding: if a.opt_str("binding")? == Some("right") { Binding::Right } else { Binding::Left },
            },
            "poster" => Layout::Poster {
                scale: a.opt_num("scale")?.unwrap_or(200.0),
                overlap: a.opt_num("overlap")?.unwrap_or(18.0),
                cut_marks: a.opt_bool("cut_marks")?.unwrap_or(true),
            },
            l => return Err(bad(format!("unknown layout {l:?}"))),
        };
        let duplex = job_duplex(a.opt_str("duplex")?.unwrap_or("off"), &layout)?;
        if a.opt_str("order")? == Some("cut-stack") && !matches!(layout, Layout::Multiple { .. }) {
            return Err(bad("cut-stack order needs layout multiple"));
        }
        let orientation = match a.opt_str("orientation")?.unwrap_or("auto") {
            "auto" => Orientation::Auto,
            "portrait" => Orientation::Portrait,
            "landscape" => Orientation::Landscape,
            o => return Err(bad(format!("unknown orientation {o:?}"))),
        };
        let content = match a.opt_str("comments_forms")?.unwrap_or("document-and-markups") {
            "document" => Content::Document,
            "document-and-markups" => Content::DocumentAndMarkups,
            "document-and-stamps" => Content::DocumentAndStamps,
            "form-fields-only" => Content::FormFieldsOnly,
            c => return Err(bad(format!("unknown comments_forms {c:?}"))),
        };
        let (paper_index, paper) = paper_named(a.opt_str("paper")?)?;
        // Print as image (Acrobat's Advanced Print Setup): pictures at `dpi`, grey when
        // `grayscale` is also asked for.
        let raster = if a.opt_bool("as_image")?.unwrap_or(false) || (portal && a.opt_bool("grayscale")?.unwrap_or(false)) {
            let dpi = a.opt_num("dpi")?.unwrap_or(300.0);
            if !(72.0..=600.0).contains(&dpi) {
                return Err(bad("dpi must be between 72 and 600"));
            }
            Some(print::Raster { dpi, grayscale: a.opt_bool("grayscale")?.unwrap_or(false) })
        } else {
            None
        };
        let settings = print::Settings { pages, paper, orientation, layout, content, raster, crops: Vec::new() };
        let sizes: Vec<(f64, f64)> = self.doc(a)?.info.pages.iter().map(|p| (p.width as f64, p.height as f64)).collect();
        let sheets = print::layout(&sizes, &settings).map_err(|e| bad(e.to_string()))?.len();
        #[cfg(target_os = "linux")]
        if portal {
            let title = self.doc(a)?.name.clone();
            let task = self.session.print_task(id, &settings).map_err(failed)?;
            if self.print_jobs.asynchronous {
                return self.print_jobs.start(task, title, sheets, settings.pages.len());
            }
            let bytes = task().map_err(failed)?;
            let outcome = (self.print_jobs.submit)(&bytes, &title, &Default::default()).map_err(failed)?;
            return Ok(json!({ "sheets": sheets, "pages": settings.pages.len(), "bytes": bytes.len(), "status": match outcome {
                print::portal::Outcome::Accepted => "accepted",
                print::portal::Outcome::Cancelled => "cancelled",
            } }));
        }
        let bytes = self.session.print_pdf(id, &settings).map_err(failed)?;
        let mut out = json!({ "sheets": sheets, "pages": settings.pages.len(), "bytes": bytes.len() });
        match (a.opt_str("path")?, a.opt_str("printer")?) {
            (Some(p), None) => {
                let path = self.resolve(p, true)?;
                write_atomic(&path, &bytes)?;
                out["path"] = json!(path.to_string_lossy());
            }
            (None, Some(printer)) => {
                let job = spool_job(a, printer, self.doc(a)?.name.clone(), duplex, paper_index)?;
                out["job"] = json!(spool::submit(&bytes, &job).map_err(|e| failed(e.to_string()))?);
            }
            _ => return Err(bad("pass either path (save the print-ready PDF) or printer (a name from printers, or \"default\")")),
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wait only inside a test; the public status tool is always nonblocking.
    #[cfg(target_os = "linux")]
    fn terminal_job(jobs: &mut PortalJobs, id: u64) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let status = jobs.inspect(id, false).unwrap();
            if status["status"] != "pending" {
                return status;
            }
            assert!(std::time::Instant::now() < deadline, "print worker did not finish");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Completed outcomes remain truthful on late cancellation, with bounded receipt storage.
    #[test]
    #[cfg(target_os = "linux")]
    fn portal_job_receipts_are_bounded_and_report_errors() {
        let mut jobs = PortalJobs { submit: std::sync::Arc::new(|_, _, _| Ok(print::portal::Outcome::Accepted)), ..Default::default() };
        for id in 1..=20 {
            let pending = jobs.start(Box::new(|| Ok(b"%PDF-1.7".to_vec())), "synthetic".into(), 1, 1).unwrap();
            assert_eq!(pending["job"], id);
            assert_eq!(terminal_job(&mut jobs, id)["status"], "accepted");
            assert_eq!(jobs.inspect(id, true).unwrap()["status"], "accepted");
        }
        assert_eq!(jobs.jobs.len(), 16);
        assert!(jobs.inspect(1, false).is_err());
        jobs.submit = std::sync::Arc::new(|_, _, _| Err("synthetic transport failure".repeat(1000)));
        let pending = jobs.start(Box::new(|| Ok(b"%PDF-1.7".to_vec())), "synthetic".into(), 1, 1).unwrap();
        let failed = terminal_job(&mut jobs, pending["job"].as_u64().unwrap());
        assert_eq!(failed["status"], "failed");
        assert!(failed["error"].as_str().unwrap().starts_with("synthetic transport failure"));
        assert!(failed["error"].as_str().unwrap().chars().count() <= 1024);
    }

    /// Only one request may be active; cancellation and dropping the session release its worker.
    #[test]
    #[cfg(target_os = "linux")]
    fn portal_jobs_cancel_and_release_the_pending_worker_on_drop() {
        let (entered_sender, entered) = std::sync::mpsc::channel();
        let (ended_sender, ended) = std::sync::mpsc::channel();
        let mut jobs = PortalJobs {
            submit: std::sync::Arc::new(move |_, _, cancel| {
                entered_sender.send(()).unwrap();
                while !cancel.is_cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                ended_sender.send(()).unwrap();
                Ok(print::portal::Outcome::Cancelled)
            }),
            ..Default::default()
        };
        let task = || Box::new(|| Ok(b"%PDF-1.7".to_vec())) as pdfcraft_engine::PrintTask;
        jobs.start(task(), "synthetic".into(), 1, 1).unwrap();
        entered.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert!(jobs.start(task(), "overlap".into(), 1, 1).is_err());
        assert_eq!(jobs.inspect(1, true).unwrap()["cancellation_requested"], true);
        assert_eq!(terminal_job(&mut jobs, 1)["status"], "cancelled");
        ended.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        jobs.start(task(), "synthetic".into(), 1, 1).unwrap();
        entered.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        drop(jobs);
        ended.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    }

    /// An unclosed portal must not hold the sequential MCP request handler or its next ping.
    #[test]
    #[cfg(all(target_os = "linux", feature = "mcp"))]
    fn pending_portal_dialog_does_not_hold_mcp_dispatch() {
        const CHILD: &str = "PDFCRAFT_PORTAL_MCP_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "printing::tests::pending_portal_dialog_does_not_hold_mcp_dispatch", "--nocapture"])
                .env(CHILD, "1")
                .env("FLATPAK_ID", "ai.storyteller.pdfcraft")
                .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/pdfcraft-test-bus")
                .output()
                .unwrap();
            assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            return;
        }
        use std::sync::{Arc, Condvar, Mutex, mpsc};
        use std::time::Duration;
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_gate = gate.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut automation = Automation::new();
            automation.print_jobs.submit = Arc::new(move |bytes, _, cancel| {
                assert!(bytes.starts_with(b"%PDF-"));
                let (lock, wake) = &*worker_gate;
                let mut released = lock.lock().unwrap();
                while !*released && !cancel.is_cancelled() {
                    released = wake.wait_timeout(released, Duration::from_millis(10)).unwrap().0;
                }
                Ok(if cancel.is_cancelled() { print::portal::Outcome::Cancelled } else { print::portal::Outcome::Accepted })
            });
            let bytes = automation.session.create_blank(200.0, 300.0, 1).unwrap();
            let doc = automation.session.open("synthetic.pdf", None, bytes, None).unwrap();
            // Direct/CLI automation retains a completed receipt instead of an orphaned job.
            let pending_submit = automation.print_jobs.submit.clone();
            automation.print_jobs.submit = Arc::new(|_, _, _| Ok(print::portal::Outcome::Accepted));
            let completed = automation.doc_print(&Args(&json!({ "doc":doc.0, "printer":"system-dialog" }))).unwrap();
            assert_eq!(completed["status"], "accepted");
            assert!(automation.print_jobs.jobs.is_empty());
            automation.print_jobs.submit = pending_submit;
            let requests = [
                json!({ "jsonrpc":"2.0", "id":1, "method":"tools/call", "params": { "name":"doc_print", "arguments": { "doc":doc.0, "printer":"system-dialog" } } }),
                json!({ "jsonrpc":"2.0", "id":2, "method":"ping" }),
                json!({ "jsonrpc":"2.0", "id":3, "method":"tools/call", "params": { "name":"doc_print_job", "arguments": { "job":1, "cancel":true } } }),
                json!({ "jsonrpc":"2.0", "id":4, "method":"ping" }),
            ];
            let input = requests.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n";
            let mut server = crate::mcp::McpServer::new(automation);
            let mut output = Vec::new();
            server.serve(std::io::Cursor::new(input), &mut output).unwrap();
            sender.send(String::from_utf8(output).unwrap()).unwrap();
        });
        let result = receiver.recv_timeout(Duration::from_secs(2));
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        worker.join().unwrap();
        let output = result.expect("MCP dispatch and its next ping must not wait for the portal dialog");
        let replies: Vec<Value> = output.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(replies.len(), 4);
        assert_eq!(replies[0]["result"]["structuredContent"]["status"], "pending");
        assert_eq!(replies[1]["result"], json!({}));
        assert_eq!(replies[2]["result"]["structuredContent"]["cancellation_requested"], true);
        assert_eq!(replies[3]["result"], json!({}));
    }

    /// #2707: a booklet sets two-sided itself whatever `duplex` asks; a bad name is still an error.
    #[test]
    fn a_booklet_decides_two_sided_itself() {
        let booklet = |subset| Layout::Booklet { subset, binding: Binding::Left };
        for asked in ["printer", "off", "long-edge", "short-edge"] {
            assert_eq!(job_duplex(asked, &booklet(BookletSubset::BothSides)).ok(), Some(Some(spool::Duplex::ShortEdge)), "{asked}");
            assert_eq!(job_duplex(asked, &booklet(BookletSubset::FrontOnly)).ok(), Some(Some(spool::Duplex::Off)), "{asked}");
            assert_eq!(job_duplex(asked, &booklet(BookletSubset::BackOnly)).ok(), Some(Some(spool::Duplex::Off)), "{asked}");
        }
        assert!(job_duplex("sideways", &booklet(BookletSubset::BothSides)).is_err());
        assert_eq!(job_duplex("long-edge", &Layout::Size(SizeMode::Fit)).ok(), Some(Some(spool::Duplex::LongEdge)), "other layouts keep it");
    }

    /// `printer` leaves two-sided to the printer's own default (#2422); cut and stack still asks
    /// for single-sided sheets, and refuses an explicit edge.
    #[test]
    fn duplex_printer_leaves_two_sided_to_the_printer() {
        assert_eq!(duplex("printer", false).ok(), Some(None));
        assert_eq!(duplex("printer", true).ok(), Some(Some(spool::Duplex::Off)));
        assert_eq!(duplex("off", false).ok(), Some(Some(spool::Duplex::Off)));
        assert_eq!(duplex("long-edge", false).ok(), Some(Some(spool::Duplex::LongEdge)));
        assert_eq!(duplex("short-edge", false).ok(), Some(Some(spool::Duplex::ShortEdge)));
        assert!(duplex("long-edge", true).is_err());
        assert!(duplex("sideways", false).is_err());
    }

    /// #3752: `doc_print` to a printer sent no paper, so the queue's default paper stood in for
    /// the one the sheets were laid out on (A4 under Letter sheets on the report's queue).
    #[test]
    fn printing_to_a_printer_feeds_the_laid_out_paper() {
        let job = |args: Value, paper_name: Option<&str>| {
            let (index, _) = paper_named(paper_name).unwrap();
            spool_job(&Args(&args), "Office", "a.pdf".into(), None, index).unwrap()
        };
        let media = |m: &str| ("media".to_string(), m.to_string());
        // US Letter when no paper is asked for, after the driver's own options.
        let letter = job(json!({ "options": { "InputSlot": "Tray3" } }), None);
        assert_eq!(letter.options, [("InputSlot".to_string(), "Tray3".to_string()), media("Letter")]);
        assert_eq!((letter.printer.as_deref(), letter.title.as_str()), (Some("Office"), "a.pdf"));
        // Every paper, by its full name, goes out as its own media name.
        for (i, (name, size)) in PAPERS.iter().enumerate() {
            assert_eq!(paper_named(Some(name)).ok(), Some((i, *size)), "{name}");
            assert_eq!(job(json!({}), Some(name)).options, print::media_option(i).into_iter().collect::<Vec<_>>(), "{name}");
        }
        // Short forms find the same paper.
        assert_eq!(job(json!({}), Some("letter")).options, [media("Letter")]);
        assert_eq!(job(json!({}), Some("ARCH D")).options, [media("na_arch-d_24x36in")]);
        assert!(matches!(paper_named(Some("Quarto")), Err(ToolError::InvalidArgs(m)) if m.contains("US Letter")));
        assert_eq!(spool_job(&Args(&json!({})), "default", String::new(), None, 0).ok().map(|j| j.printer), Some(None));
        assert!(spool_job(&Args(&json!({ "options": { "InputSlot": 3 } })), "Office", String::new(), None, 0).is_err());
    }
}
