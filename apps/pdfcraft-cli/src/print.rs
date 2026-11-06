//! `pdfcraft-cli print --wsd URL FILE ... --yes`: send a file to a WSD network printer.
//!
//! A front for the `print_wsd` and `wsd_printer_info` tools. The printer URL must be a numeric
//! loopback, link-local or private address, nothing is sent without `--yes`, and builds without
//! the `network-print` feature refuse. A PDF is opened as a document (so a printer without PDF support
//! gets PWG raster, when it lists it); any other file is sent as it is. The printer accepts the
//! document, then the job is followed until it ends or `--wait` runs out; nothing is sent twice.

use serde_json::json;

use crate::{CliError, OptionSpec, automation, print_output, validate_options};

const OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--wsd", takes_value: true },
    OptionSpec { name: "--yes", takes_value: false },
    OptionSpec { name: "--info", takes_value: false },
    OptionSpec { name: "--root", takes_value: true },
    OptionSpec { name: "--format", takes_value: true },
    OptionSpec { name: "--copies", takes_value: true },
    OptionSpec { name: "--duplex", takes_value: true },
    OptionSpec { name: "--orientation", takes_value: true },
    OptionSpec { name: "--paper", takes_value: true },
    OptionSpec { name: "--quality", takes_value: true },
    OptionSpec { name: "--dpi", takes_value: true },
    OptionSpec { name: "--color", takes_value: true },
    OptionSpec { name: "--job-name", takes_value: true },
    OptionSpec { name: "--strict", takes_value: false },
    OptionSpec { name: "--ticket", takes_value: true },
    OptionSpec { name: "--drop-ticket-settings", takes_value: false },
    OptionSpec { name: "--wait", takes_value: true },
    OptionSpec { name: "--password", takes_value: true },
];

/// Option → tool argument, for the value-taking options that map one to one.
const VALUE_ARGS: &[(&str, &str, bool)] = &[
    ("--format", "format", false),
    ("--copies", "copies", true),
    ("--duplex", "duplex", false),
    ("--orientation", "orientation", false),
    ("--paper", "paper", false),
    ("--quality", "quality", false),
    ("--dpi", "dpi", true),
    ("--color", "color", false),
    ("--job-name", "job_name", false),
    ("--ticket", "ticket", false),
    ("--wait", "wait_secs", true),
];

pub(super) fn run(args: &[String]) -> Result<(), CliError> {
    validate_options("print", args, OPTIONS)?;
    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if let Some(spec) = OPTIONS.iter().find(|s| s.name == arg) {
            if !seen.insert(spec.name) {
                return Err(format!("print: {} may only be supplied once", spec.name).into());
            }
            if spec.takes_value {
                if args.get(i + 1).is_none_or(|v| v.is_empty() || v.starts_with("--")) {
                    return Err(format!("print: {} needs a value", spec.name).into());
                }
                i += 1;
            }
        } else {
            files.push(arg.as_str());
        }
        i += 1;
    }
    let value = |name: &str| -> Option<&str> { args.iter().position(|a| a == name).and_then(|p| args.get(p + 1)).map(String::as_str) };
    let url = value("--wsd").ok_or("print: missing --wsd URL (the printer's address, for example http://192.168.1.50:5357/service); other printers are printed to from the print dialog")?;
    // The address rules first: a refused printer never gets as far as opening the file.
    pdfcraft_print::network::wsd::PrinterUrl::parse(url).map_err(|e| format!("print: {e}"))?;
    let mut auto = automation(args)?;
    if seen.contains("--info") {
        if !files.is_empty() {
            return Err("print: --info takes no file".into());
        }
        let content = auto.call("wsd_printer_info", &json!({ "printer_url": url })).map_err(|e| e.to_string())?;
        return print_output(&auto, content, None);
    }
    let [file] = files.as_slice() else {
        return Err("print: give exactly one FILE to print (or --info to only read the printer)".into());
    };
    if !seen.contains("--yes") {
        return Err(format!("print: this would send {file} to the printer at {url}; add --yes to confirm").into());
    }
    let mut params = json!({ "printer_url": url, "user_confirmed": true });
    for (option, key, number) in VALUE_ARGS {
        if let Some(v) = value(option) {
            params[*key] = if *number { json!(v.parse::<i64>().map_err(|_| format!("print: {option} needs a whole number"))?) } else { json!(v) };
        }
    }
    if seen.contains("--strict") {
        params["strict"] = json!(true);
    }
    if seen.contains("--drop-ticket-settings") {
        params["drop_ticket_settings"] = json!(true);
    }
    let is_pdf = value("--format").map_or_else(|| file.to_ascii_lowercase().ends_with(".pdf"), |f| f.eq_ignore_ascii_case("application/pdf"));
    if is_pdf {
        let mut open = json!({ "path": file });
        if let Some(p) = value("--password") {
            open["password"] = json!(p);
        }
        let opened = auto.call("doc_open", &open).map_err(|e| e.to_string())?;
        let doc = opened
            .iter()
            .find_map(|c| match c {
                pdfcraft_automation::Content::Json(v) => v["doc"].as_u64(),
                _ => None,
            })
            .ok_or("print: the document did not open")?;
        params["doc"] = json!(doc);
    } else {
        params["path"] = json!(file);
    }
    let content = auto.call("print_wsd", &params).map_err(|e| e.to_string())?;
    let verdict = verdict(&content);
    // A closed report pipe is the usual quiet exit for a job that went well, but it must not hide a
    // failed or unconfirmed one: those keep their non-zero verdict whatever happened to the report.
    finish(verdict, print_output(&auto, content, None))
}

/// The command's result from the print verdict and what became of writing the report.
fn finish(verdict: Verdict, written: Result<(), CliError>) -> Result<(), CliError> {
    match verdict {
        Verdict::Done => written,
        Verdict::Failed => Err("print: the job did not complete successfully; see the JSON report".into()),
        Verdict::Unconfirmed => Err(UNCONFIRMED.into()),
    }
}

/// What a print that ended without a confirmed result tells the person: the document may well have printed.
const UNCONFIRMED: &str = "print: the printer accepted the document, but how the job ended is unknown (it may be printing, or may have printed); check the printer before sending again; see the JSON report";

/// How the `print_wsd` report ended the command.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Done,
    /// A terminal failure (refused, canceled, aborted, completed with errors…): the job did not print.
    Failed,
    /// The printer has the document but its end state is unknown (`still_running`, `accepted_unknown`,
    /// `forgotten`, or the job could not be followed): sending it again could print it twice. Still a
    /// non-zero exit, as the CLI has no third status: the message and the report's `outcome` tell it apart.
    Unconfirmed,
}

fn verdict(content: &[pdfcraft_automation::Content]) -> Verdict {
    let reports = content.iter().filter_map(|c| match c {
        pdfcraft_automation::Content::Json(v) => Some(v),
        _ => None,
    });
    let mut verdict = Verdict::Done;
    for v in reports {
        if v["success"] != false {
            continue;
        }
        let unconfirmed =
            v["accepted"] == true && matches!(v["outcome"].as_str(), Some("still_running" | "accepted_unknown" | "forgotten" | "failed"));
        if !unconfirmed {
            return Verdict::Failed;
        }
        verdict = Verdict::Unconfirmed;
    }
    verdict
}

#[cfg(test)]
mod tests {
    use pdfcraft_automation::Content;
    use serde_json::json;

    use super::{CliError, Verdict, finish, verdict};

    fn report(accepted: bool, outcome: &str, success: bool) -> Vec<Content> {
        vec![Content::Json(json!({ "accepted": accepted, "outcome": outcome, "success": success }))]
    }

    #[test]
    fn accepted_but_unconfirmed_outcomes_are_not_a_plain_failure() {
        assert_eq!(verdict(&report(true, "completed", true)), Verdict::Done);
        for outcome in ["still_running", "accepted_unknown", "forgotten"] {
            assert_eq!(verdict(&report(true, outcome, false)), Verdict::Unconfirmed, "{outcome}");
        }
        for outcome in ["aborted", "canceled", "completed_with_errors", "unknown"] {
            assert_eq!(verdict(&report(true, outcome, false)), Verdict::Failed, "{outcome}");
        }
        assert_eq!(verdict(&report(false, "failed", false)), Verdict::Failed);
    }

    fn broken_pipe() -> Result<(), CliError> {
        Err(CliError::Stdout(std::io::Error::from(std::io::ErrorKind::BrokenPipe)))
    }

    #[test]
    fn a_closed_report_pipe_never_hides_a_failed_or_unconfirmed_print() {
        // The job went well: a closed pipe keeps its quiet handling (main turns it into success).
        assert!(matches!(finish(Verdict::Done, broken_pipe()), Err(CliError::Stdout(_))));
        assert!(finish(Verdict::Done, Ok(())).is_ok());
        // The job did not: the verdict is a message error (non-zero exit), not the pipe error.
        for verdict in [Verdict::Failed, Verdict::Unconfirmed] {
            assert!(matches!(finish(verdict, broken_pipe()), Err(CliError::Message(_))));
        }
    }
}
