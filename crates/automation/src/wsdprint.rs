//! WS-Print tools: ask a network printer what it is (`wsd_printer_info`) and send it a document
//! (`print_wsd`), through craft-print's WS-Print backend (`pdfcraft_print::network`).
//!
//! Network policy (craft-wsd's, through craft-print): the printer URL must be a numeric loopback,
//! link-local or private address; nothing is sent without `user_confirmed: true`; paths obey
//! `--root`. The tool returns the job's end state, or `still_running` / `accepted_unknown` when the
//! printer accepted the document and the wait ran out or the printer cannot report. It never sends
//! a document twice: a failure that may have reached the printer says so. Tests inject their own
//! transport with `Automation::with_wsd_transport`. Without the `network-print` feature (and on
//! wasm32) both tools refuse.

#[cfg(not(all(feature = "network-print", not(target_arch = "wasm32"))))]
mod off {
    use serde_json::Value;

    use crate::{Args, Automation, Result, failed};

    const OFF: &str = "this build of PdfCraft has no network printing (built without the `network-print` feature)";

    impl Automation {
        pub(crate) fn wsd_printer_info(&self, _: &Args) -> Result<Value> {
            Err(failed(OFF))
        }

        pub(crate) fn print_wsd(&mut self, _: &Args) -> Result<Value> {
            Err(failed(OFF))
        }
    }
}

#[cfg(all(feature = "network-print", not(target_arch = "wasm32")))]
pub(crate) use on::WsdTransport;

#[cfg(all(feature = "network-print", not(target_arch = "wasm32")))]
mod on {
    use std::io::Read;
    use std::time::Duration;

    use pdfcraft_engine::print::{self as layout, PAPERS, Subset};
    use pdfcraft_print::network::wsd::{self as wsd, TicketShape, Transfer};
    use pdfcraft_print::network::{
        self, CancelToken, ColourMode, Document, DocumentFormat, Duplex, ErrorKind, Job, JobOptions, Orientation, PrintError, Printer, Quality,
        Resolution, Settings, UserConfirmed, WaitPolicy, WatchOutcome,
    };
    use serde_json::{Value, json};

    use crate::{Args, Automation, Result, ToolError, failed};

    /// The most a file sent as-is may be.
    const MAX_FILE: u64 = 256 << 20;

    pub(crate) type WsdTransport = network::Transport;

    fn bad(m: impl Into<String>) -> ToolError {
        ToolError::InvalidArgs(m.into())
    }

    /// A refusal the caller can fix (an address, a setting, the missing confirmation) is an argument
    /// error; the rest failed. A failure that may have reached the printer says to check it first.
    fn print_err(e: PrintError) -> ToolError {
        match e.kind() {
            ErrorKind::InvalidRequest | ErrorKind::ConsentRequired => bad(e.message().to_owned()),
            _ if e.outcome_unknown() => failed(format!("{}; {}", e.message(), network::CHECK_THE_PRINTER)),
            _ => failed(e.message().to_owned()),
        }
    }

    impl Automation {
        /// Use `transport` for WS-Print instead of the built-in HTTP one (tests; embedding).
        pub fn with_wsd_transport(mut self, transport: WsdTransport) -> Self {
            self.wsd = Some(transport);
            self
        }

        fn wsd_printer(&self, url: &str, settings: Settings) -> Result<Printer> {
            Printer::open_with(url, self.wsd.clone(), settings).map_err(print_err)
        }

        pub(crate) fn wsd_printer_info(&self, a: &Args) -> Result<Value> {
            let printer = self.wsd_printer(a.str("printer_url")?, Settings::default())?;
            let info = printer.info().map_err(print_err)?;
            let mut out = json!({ "printer_url": printer.address(), "invalid_sections": info.invalid });
            if let Some(d) = &info.description {
                out["description"] = json!({
                    "name": d.name,
                    "color_supported": d.color_supported,
                    "multiple_documents": d.multiple_documents,
                    "pages_per_minute": d.pages_per_minute,
                });
            }
            if let Some(s) = &info.status {
                out["status"] = json!({ "state": s.state, "queued_jobs": s.queued_jobs, "reasons": s.reasons });
            }
            if let Some(c) = &info.capabilities {
                out["capabilities"] = json!({
                    "formats": c.formats,
                    "compressions": c.compressions,
                    "media_sizes": c.media_sizes,
                    "media_types": c.media_types,
                    "orientations": c.orientations,
                    "qualities": c.qualities,
                    "sides": c.sides,
                    "copies": c.copies.map(|(min, max)| json!({ "min": min, "max": max })),
                    "resolutions": c.resolutions.iter().map(|(w, h)| json!([w, h])).collect::<Vec<_>>(),
                });
            }
            Ok(out)
        }

        pub(crate) fn print_wsd(&mut self, a: &Args) -> Result<Value> {
            // The person's yes comes first: nothing else is looked at, let alone sent, without it.
            let confirmed = UserConfirmed::attest(a.opt_bool("user_confirmed")?).map_err(print_err)?;
            let (settings, job, mut options, wait) = print_options(a)?;
            let printer = self.wsd_printer(a.str("printer_url")?, settings)?;
            let (doc_arg, path_arg) = (a.opt_int("doc")?, a.opt_str("path")?);
            let bytes_file = match (doc_arg, path_arg) {
                (Some(_), Some(_)) | (None, None) => return Err(bad("pass exactly one of doc (an open document) or path (a file to send as it is)")),
                (None, Some(p)) => Some(self.read_printable(p, a.opt_str("format")?)?),
                (Some(_), None) => None,
            };
            let cancel = CancelToken::new();
            let sent = match bytes_file {
                Some((format, bytes)) => printer.submit(Document::new(format, &bytes), job, options, confirmed, &cancel, &mut |_| {}),
                None => {
                    let id = self.doc(a)?.id;
                    let paper = options.media.as_deref().and_then(sheet_for_media).unwrap_or(PAPERS[0].1);
                    let count = self.doc(a)?.info.pages.len();
                    let labels: Vec<String> = self.doc(a)?.info.pages.iter().map(|p| p.label.clone()).collect();
                    let pages = layout::select_pages(count, None, &labels, Subset::All, false).map_err(|e| bad(e.to_string()))?;
                    let sheets = layout::Settings { pages, paper, ..Default::default() };
                    // Also enforces the document's printing permission.
                    let pdf = self.session.print_pdf(id, &sheets).map_err(failed)?;
                    // The page count is what progress and status use for a total (WS-Print reports none).
                    options.expected_pages = u32::try_from(sheets.pages.len()).ok();
                    // A printer without PDF is sent PWG raster: the registry converts through the
                    // registered converter (PdfCraft's renderer), at a resolution the printer lists.
                    printer.submit(Document::pdf(&pdf), job, options, confirmed, &cancel, &mut |_| {})
                }
            }
            .map_err(print_err)?;
            // Accepted. Following the job is a separate, bounded step; the document is not sent again.
            let policy = WaitPolicy { interval: Some(Duration::from_millis(500)), ..WaitPolicy::default() }.with_max_wait(wait);
            let mut watch = printer.watch(&sent, policy).map_err(print_err)?;
            let outcome = watch.run(printer.source(), &mut |_| {});
            let last = match &outcome {
                WatchOutcome::Terminal(s) => Some(s),
                WatchOutcome::Forgotten { last } | WatchOutcome::TimedOut { last } => last.as_ref(),
                _ => watch.last_status(),
            };
            let mut out = json!({
                "accepted": true,
                "job": sent.job.to_string(),
                "job_id": sent.job_number,
                "outcome": network::outcome_name(&outcome),
                "success": matches!(&outcome, WatchOutcome::Terminal(s) if s.state.is_success()),
                "state": last.map(|s| s.state.name()),
                "reasons": last.map(|s| s.reasons.clone()).unwrap_or_default(),
                "format": sent.format,
                "converted_from": sent.conversion.as_ref().map(|c| c.from.mime()),
                "resolution": sent.conversion.as_ref().map(|c| json!([c.resolution.x_dpi, c.resolution.y_dpi])),
                "bytes_sent": sent.bytes_sent,
                "sheets_completed": last.and_then(|s| s.sheets_done).unwrap_or(0),
                "printer": sent.printer_name,
                "warnings": sent.warnings,
            });
            if let WatchOutcome::Failed(e) = &outcome {
                out["error"] = json!(e.message());
            }
            Ok(out)
        }

        /// A file to send as it is: its format (given as a MIME type, else what its bytes and name
        /// reveal) and its bytes (capped).
        fn read_printable(&self, path: &str, format: Option<&str>) -> Result<(DocumentFormat, Vec<u8>)> {
            let resolved = self.resolve(path, false)?;
            let file = std::fs::File::open(&resolved).map_err(|e| failed(format!("{path}: {e}")))?;
            let mut bytes = Vec::new();
            file.take(MAX_FILE + 1).read_to_end(&mut bytes).map_err(|e| failed(format!("{path}: {e}")))?;
            if bytes.len() as u64 > MAX_FILE {
                return Err(failed(format!("{path} is larger than {} MB", MAX_FILE >> 20)));
            }
            let format = match format {
                Some(f) => DocumentFormat::from_mime(f).ok_or_else(|| bad(format!("unknown format {f:?}: use one of {}", known_formats())))?,
                None => DocumentFormat::detect(resolved.file_name().and_then(|n| n.to_str()), &bytes)
                    .ok_or_else(|| bad(format!("cannot tell the document format from the file: pass format (one of {})", known_formats())))?,
            };
            Ok((format, bytes))
        }
    }

    fn known_formats() -> String {
        DocumentFormat::ALL.iter().map(|f| f.mime()).collect::<Vec<_>>().join(", ")
    }

    /// The print-ready sheet size for a `MediaSizeName`, when it is one of the common ones.
    fn sheet_for_media(name: &str) -> Option<(f64, f64)> {
        let find = |label: &str| PAPERS.iter().find(|(n, _)| *n == label).map(|(_, s)| *s);
        match name.split('_').take(2).collect::<Vec<_>>().as_slice() {
            ["iso", "a3"] => find("A3"),
            ["iso", "a4"] => find("A4"),
            ["iso", "a5"] => find("A5"),
            ["na", "letter"] => find("US Letter"),
            ["na", "legal"] => find("US Legal"),
            _ => None,
        }
    }

    /// What the backend keeps for itself, the job and its per-job options, and how long to follow the
    /// job after the printer accepts it.
    fn print_options(a: &Args) -> Result<(Settings, Job, JobOptions, Duration)> {
        let mut job = Job::default();
        if let Some(n) = a.opt_str("job_name")? {
            n.clone_into(&mut job.title);
        }
        job.copies = match a.opt_int("copies")? {
            None => 1,
            Some(n) => u32::try_from(n)
                .ok()
                .filter(|n| (1..=wsd::ticket::MAX_COPIES).contains(n))
                .ok_or_else(|| bad(format!("copies must be 1 to {}", wsd::ticket::MAX_COPIES)))?,
        };
        job.duplex = match a.opt_str("duplex")? {
            None | Some("off") => None,
            Some("long-edge") => Some(Duplex::LongEdge),
            Some("short-edge") => Some(Duplex::ShortEdge),
            Some(d) => return Err(bad(format!("unknown duplex {d:?}; use off, long-edge or short-edge"))),
        };
        let mut o = JobOptions::new();
        if let Some(v) = a.opt_str("orientation")? {
            o = o.with_orientation(match v {
                "portrait" => Orientation::Portrait,
                "landscape" => Orientation::Landscape,
                "reverse-portrait" => Orientation::ReversePortrait,
                "reverse-landscape" => Orientation::ReverseLandscape,
                v => return Err(bad(format!("unknown orientation {v:?}"))),
            });
        }
        if let Some(v) = a.opt_str("quality")? {
            o = o.with_quality(match v {
                "draft" => Quality::Draft,
                "normal" => Quality::Normal,
                "high" => Quality::High,
                "photo" => Quality::Photo,
                v => return Err(bad(format!("unknown quality {v:?}; use draft, normal, high or photo"))),
            });
        }
        if let Some(m) = a.opt_str("paper")? {
            o = o.with_media(m);
        }
        if let Some(d) = a.opt_int("dpi")? {
            let d = u32::try_from(d).ok().filter(|d| (50..=2400).contains(d)).ok_or_else(|| bad("dpi must be 50 to 2400"))?;
            o = o.with_resolution(Resolution::square(d));
        }
        o = o.with_colour(match a.opt_str("color")?.unwrap_or("auto") {
            "auto" => ColourMode::Auto,
            "color" => ColourMode::Colour,
            "mono" => ColourMode::Mono,
            v => return Err(bad(format!("unknown color {v:?}; use auto, color or mono"))),
        });
        let settings = Settings {
            strict: a.opt_bool("strict")?.unwrap_or(false),
            drop_ticket_settings: a.opt_bool("drop_ticket_settings")?.unwrap_or(false),
            ticket_shape: match a.opt_str("ticket")? {
                None => TicketShape::Full,
                Some(t) => TicketShape::parse(t).ok_or_else(|| bad(format!("unknown ticket {t:?}; use full, description, job_name or empty")))?,
            },
            transfer: match a.opt_str("transfer")?.unwrap_or("mtom") {
                "mtom" => Transfer::Mtom,
                "inline" => Transfer::InlineBase64,
                v => return Err(bad(format!("unknown transfer {v:?}; use mtom or inline"))),
            },
        };
        let wait = a.opt_int("wait_secs")?.unwrap_or(120);
        let wait = u64::try_from(wait).ok().filter(|w| *w <= 3600).ok_or_else(|| bad("wait_secs must be 0 to 3600"))?;
        Ok((settings, job, o, Duration::from_secs(wait)))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn media_names_pick_a_sheet() {
            assert_eq!(sheet_for_media("iso_a4_210x297mm"), Some(PAPERS[4].1));
            assert_eq!(sheet_for_media("na_letter_8.5x11in"), Some(PAPERS[0].1));
            assert_eq!(sheet_for_media("jis_b4_257x364mm"), None);
            assert_eq!(sheet_for_media(""), None);
        }
    }
}
