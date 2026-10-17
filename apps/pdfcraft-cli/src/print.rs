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
    let failed = content.iter().any(|c| matches!(c, pdfcraft_automation::Content::Json(v) if v["success"] == false));
    print_output(&auto, content, None)?;
    if failed { Err("print: the job did not complete successfully; see the JSON report".into()) } else { Ok(()) }
}
