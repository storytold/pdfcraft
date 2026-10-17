//! Scanner listing and acquire. Listing does not feed paper.

use std::path::Path;

use pdfcraft_platform::scan;
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed};

impl Automation {
    pub(crate) fn scan_devices(&self, args: &Args) -> Result<Value> {
        list_scan_devices(args.opt_str("device")?, args.opt_bool("details")?.unwrap_or(false))
    }

    pub(crate) fn scan_acquire(&self, args: &Args) -> Result<Value> {
        let out = args.str("out")?;
        let folder = self.resolve(out, true)?;
        std::fs::create_dir_all(&folder).map_err(|error| failed(format!("{out}: {error}")))?;
        let settings = scan_settings_from_args(args)?;
        settings.checked().map_err(|error| failed(error.to_string()))?;
        match scan::acquire(&settings) {
            Ok(acquired) => finish_acquire_json(&folder, &settings, &acquired, false, None),
            Err(error) => acquire_failure(&folder, &settings, error),
        }
    }
}

impl Automation {
    /// `scan_jobs`: list or show the journals under a folder. Read-only; honours the root like any path.
    pub(crate) fn scan_jobs(&self, args: &Args) -> Result<Value> {
        use crate::scan_jobs as jobs;
        let dir = args.opt_str("dir")?.unwrap_or(".");
        let folder = self.resolve(dir, false)?;
        if args.opt_bool("show")?.unwrap_or(false) {
            let record = jobs::read_record(&folder).map_err(failed)?;
            return Ok(json!({ "job": jobs::job_json(&folder, &record), "text": jobs::detail_text(jobs::PROGRAM, &folder, &record) }));
        }
        // The folder and its immediate subfolders; a subfolder that resolves outside the folder (a link) is left out.
        let real = folder.canonicalize().map_err(|error| failed(format!("{dir}: {error}")))?;
        let mut candidates = jobs::candidate_dirs(&[], &folder);
        candidates.retain(|candidate| candidate == &folder || candidate.canonicalize().is_ok_and(|path| path.starts_with(&real)));
        let (lines, rows) = jobs::jobs_listing(&candidates, &folder);
        Ok(json!({ "count": rows.len(), "jobs": rows, "lines": lines }))
    }
}

/// How a page got its length: `{source, length_mils, length_inches, length_mm, approximate}`. `source` is `driver`,
/// `device`, `size` or `inferred`; only `inferred` is approximate. The lengths are `null` when not known.
pub fn scan_length_json(record: &scan::LengthRecord) -> Value {
    json!({
        "source": record.source.as_str(),
        "length_mils": record.length_mils,
        "length_inches": record.length_inches().map(|inches| (inches * 1000.0).round() / 1000.0),
        "length_mm": record.length_mm().map(|mm| (mm * 10.0).round() / 10.0),
        "approximate": record.approximate,
    })
}

/// What an item publishes about page length: `{device_detect, max_length_mils}`; `device_detect` is `unknown`,
/// `absent` or `present`.
fn page_length_support_json(support: scan::PageLengthSupport) -> Value {
    json!({ "device_detect": support.device_detect.as_str(), "max_length_mils": support.max_length_mils })
}

/// `{kind, phase, hresult, code_name, retryable, resumable, message}` for a failed scan: the
/// identity the engine settled on, so a caller reads fields and never the sentence. `hresult` is
/// hex text (`0x80210006`); absent values are `null`. The vocabulary is `craft-scan`'s; `hresult` and
/// `code_name` come from a backend error code in the `hresult` domain (the Windows backend).
pub fn scan_error_info_json(info: &scan::ScanErrorInfo) -> Value {
    json!({
        "kind": info.kind.as_str(),
        "phase": info.phase.as_str(),
        "hresult": info.hresult_hex(),
        "code_name": info.code_name(),
        "retryable": info.retryable,
        "resumable": info.resumable,
        "message": info.message,
    })
}

/// A failed acquire: pages that arrived are saved and reported as `partial` with `error` and
/// `error_info`; with none, the call fails and the failure's data rides along.
fn acquire_failure(folder: &Path, settings: &scan::ScanSettings, error: scan::ScanError) -> Result<Value> {
    let info = scan::ScanErrorInfo::from_acquire(&error, settings);
    match error {
        scan::ScanError::Incomplete { message, acquired, .. } => {
            let mut out = finish_acquire_json(folder, settings, &acquired, true, Some(message))?;
            out["error_info"] = scan_error_info_json(&info);
            Ok(out)
        }
        other => {
            let message = match other {
                scan::ScanError::Unsupported => scan::ScanError::Unsupported.to_string(),
                scan::ScanError::Failed(message) | scan::ScanError::Native { message, .. } => message,
                scan::ScanError::Incomplete { message, .. } => message,
            };
            Err(ToolError::FailedWith { message, data: scan_error_info_json(&info) })
        }
    }
}

fn scan_settings_from_args(args: &Args) -> Result<scan::ScanSettings> {
    let pages = if let Some(n) = args.opt_int("pages")? {
        u32::try_from(n).map_err(|_| ToolError::InvalidArgs("pages must be non-negative".into()))?
    } else if let Some(text) = args.opt_str("pages")? {
        if text == "all" { 0 } else { text.parse::<u32>().map_err(|_| ToolError::InvalidArgs("pages must be a whole number or all".into()))? }
    } else {
        1
    };
    let color_text = args.opt_str("color")?.unwrap_or("gray");
    let color = scan::Color::parse(color_text).ok_or_else(|| ToolError::InvalidArgs(format!("color must be gray, rgb, or bw (got {color_text})")))?;
    let source = scan::Source::parse(args.opt_str("source")?.unwrap_or("auto"))
        .ok_or_else(|| ToolError::InvalidArgs("source must be auto, feeder, or glass".into()))?;
    let dpi = args.opt_int("dpi")?.unwrap_or(200);
    if dpi < 1 {
        return Err(ToolError::InvalidArgs("dpi must be positive".into()));
    }
    Ok(scan::ScanSettings {
        device: args.opt_str("device")?.unwrap_or("").to_string(),
        source,
        dpi: u32::try_from(dpi).map_err(|_| ToolError::InvalidArgs("dpi out of range".into()))?,
        dpi_y: args.opt_int("dpi_y")?.map(|n| u32::try_from(n).map_err(|_| ToolError::InvalidArgs("dpi_y out of range".into()))).transpose()?,
        color,
        pages,
        brightness: None,
        contrast: None,
        threshold: None,
        rotation: None,
        region: None,
        format: args.opt_str("format")?.map(str::to_string),
        compression: None,
        intent: None,
        page_size: args
            .opt_str("page_size")?
            .map(|text| {
                scan::PageSize::parse(text).ok_or_else(|| {
                    ToolError::InvalidArgs(format!("page_size must be a4, letter, legal, auto, custom, or business-card (got {text})"))
                })
            })
            .transpose()?,
        page_length: match args.opt_str("page_length")? {
            None => scan::PageLength::Driver,
            Some(text) => scan::PageLength::parse(text).ok_or_else(|| {
                ToolError::InvalidArgs(format!("page_length must be driver, size, auto, or infer (infer is approximate) (got {text})"))
            })?,
        },
        auto_deskew: None,
        orientation: None,
        photometric: None,
        invert: None,
        mirror: None,
        lamp: None,
        duplex: args.opt_bool("duplex")?.unwrap_or(false),
        cancel: None,
        progress: None,
    })
}

fn finish_acquire_json(
    folder: &Path,
    settings: &scan::ScanSettings,
    acquired: &scan::Acquired,
    partial: bool,
    error: Option<String>,
) -> Result<Value> {
    let paths = scan::write_pages(folder, &acquired.pages).map_err(failed)?;
    let pages: Vec<Value> = paths
        .iter()
        .zip(acquired.pages.iter())
        .map(|(path, page)| {
            json!({
                "path": path.display().to_string(),
                "format": page.format,
                "width": page.width,
                "height": page.height,
                "dpi": page.dpi,
                "dpi_y": page.dpi_y,
                "requested_dpi": page.requested_dpi,
                "bytes": page.bytes.len(),
                "thresholded": page.thresholded,
                "length": scan_length_json(&page.length),
            })
        })
        .collect();
    let mut out = json!({
        "source": acquired.source.as_str(),
        "requested_source": settings.source.as_str(),
        "reverse": false,
        "partial": partial,
        "count": pages.len(),
        "pages": pages,
    });
    if let Some(message) = error {
        out["error"] = json!(message);
    }
    Ok(out)
}

/// Scanner listing.
///
/// By default nothing is opened: each row is `{id, name, port, connection, details: "skipped"}`.
/// `filter` (case-insensitive id, name or port substring) opens only the matching devices and
/// returns only those, with `details: "loaded"` and the full capability keys. `details == true`
/// without a filter opens every device. A device that fails to open stays in the list as
/// `details: "error"` with the engine's own `error` sentence.
pub fn list_scan_devices(filter: Option<&str>, details: bool) -> Result<Value> {
    check_filter(filter)?;
    let (listed, errors) = listed_refs()?;
    let mut out = build_listing(filter, details, &listed, &mut |id| scan::scanner_details(id))?;
    if !errors.is_empty() {
        out["errors"] = Value::Array(errors);
    }
    Ok(out)
}

/// The scanners the backends list, and one `{backend, error}` row for each backend that failed
/// while another answered. When every backend failed, that is the error.
fn listed_refs() -> Result<(Vec<scan::ScannerRef>, Vec<Value>)> {
    refs_from(scan::scanner_list().map_err(failed)?)
}

fn refs_from(listing: scan::Listing<scan::ScannerRef>) -> Result<(Vec<scan::ScannerRef>, Vec<Value>)> {
    if listing.items.is_empty()
        && let Some(first) = listing.errors.first()
    {
        return Err(failed(first.error.to_string()));
    }
    let errors = listing.errors.iter().map(|failure| json!({ "backend": failure.backend, "error": failure.error.to_string() })).collect();
    Ok((listing.items, errors))
}

/// One device by exact id (`wia:<id>`), as a `details` row (`loaded`, or `error` when it will not open).
/// Opens that device only. An unknown id is an error.
pub fn scan_device_details(id: &str) -> Result<Value> {
    check_filter(Some(id))?;
    let (listed, _) = listed_refs()?;
    detail_row_for(id, &listed, &mut |id| scan::scanner_details(id))
}

type OpenScanner<'a> = dyn FnMut(&str) -> std::result::Result<scan::Scanner, scan::ScanError> + 'a;

fn check_filter(filter: Option<&str>) -> Result<()> {
    let Some(text) = filter else {
        return Ok(());
    };
    if text.trim().is_empty() {
        return Err(ToolError::InvalidArgs("the device filter must not be empty".into()));
    }
    Ok(())
}

fn build_listing(filter: Option<&str>, details: bool, listed: &[scan::ScannerRef], open: &mut OpenScanner<'_>) -> Result<Value> {
    check_filter(filter)?;
    let selected: Vec<&scan::ScannerRef> = listed.iter().filter(|item| filter.is_none_or(|text| item.matches(text))).collect();
    let open_them = details || filter.is_some();
    let rows: Vec<Value> = selected
        .into_iter()
        .map(|item| {
            if !open_them {
                ref_row(item, scan::ListDetails::Skipped)
            } else {
                opened_row(item, open)
            }
        })
        .collect();
    Ok(json!({ "count": rows.len(), "scanners": rows }))
}

fn detail_row_for(id: &str, listed: &[scan::ScannerRef], open: &mut OpenScanner<'_>) -> Result<Value> {
    let item = listed.iter().find(|item| item.id == id).ok_or_else(|| failed(format!("no scanner with id {id:?}")))?;
    Ok(opened_row(item, open))
}

fn ref_row(item: &scan::ScannerRef, details: scan::ListDetails) -> Value {
    json!({ "id": item.id, "name": item.name, "port": item.port, "connection": item.connection.as_str(), "details": details.as_str() })
}

fn opened_row(item: &scan::ScannerRef, open: &mut OpenScanner<'_>) -> Value {
    opened_pair(item, open).0
}

/// The row for one opened device, and the scanner itself when it opened.
fn opened_pair(item: &scan::ScannerRef, open: &mut OpenScanner<'_>) -> (Value, Option<scan::Scanner>) {
    match open(&item.id) {
        Ok(scanner) => {
            let mut row = scanner_json(&scanner);
            if let Some(map) = row.as_object_mut() {
                map.insert("details".into(), json!(scan::ListDetails::Loaded.as_str()));
            }
            (row, Some(scanner))
        }
        Err(error) => {
            let mut row = ref_row(item, scan::ListDetails::Error);
            if let Some(map) = row.as_object_mut() {
                map.insert("error".into(), json!(error.to_string()));
            }
            (row, None)
        }
    }
}

fn scanner_json(scanner: &scan::Scanner) -> Value {
    let status = scanner.handling_status;
    json!({
        "id": scanner.id,
        "name": scanner.name,
        "feeder": scanner.feeder,
        "flatbed": scanner.flatbed,
        "duplex": scanner.duplex,
        "feeder_ready": scanner.feeder_ready,
        "handling_status": status_json(&status),
        "conditions": status.conditions(),
        "feeder_capacity": scanner.feeder_capacity,
        "connection": scanner.connection.as_str(),
        "port": scanner.port,
        "optical_x": scanner.optical_x,
        "optical_y": scanner.optical_y,
        "pages": span_json(scanner.pages),
        "bed": area_json(scanner.bed),
        "sheet": area_json(scanner.sheet),
        "glass": choices_json(scanner.glass.as_ref()),
        "feeder_choices": choices_json(scanner.feeder_choices.as_ref()),
        "page_length": {
            "glass": page_length_support_json(scanner.length_support(scan::Source::Glass)),
            "feeder": page_length_support_json(scanner.length_support(scan::Source::Feeder)),
        },
    })
}

/// All nine handling flags, by the engine's own list of names.
fn status_json(status: &scan::HandlingStatus) -> Value {
    Value::Object(status.flags().into_iter().map(|(name, on)| (name.to_string(), Value::Bool(on))).collect())
}

fn choices_json(choices: Option<&scan::ImageChoices>) -> Value {
    let Some(choices) = choices else {
        return Value::Null;
    };
    json!({
        "dpi": choices.dpi,
        "dpi_y": choices.dpi_y,
        "colors": choices.colors.iter().map(|color| color.as_str()).collect::<Vec<_>>(),
        "brightness": span_json(choices.brightness),
        "contrast": span_json(choices.contrast),
        "threshold": span_json(choices.threshold),
        "rotation": choices.rotation.iter().map(|turn| turn.name()).collect::<Vec<_>>(),
        "formats": choices.formats,
        "compression": choices.compression,
        "intents": choices.intents.iter().map(|intent| intent.as_str()).collect::<Vec<_>>(),
        "page_sizes": choices.page_sizes.iter().map(|size| size.as_str()).collect::<Vec<_>>(),
        "auto_deskew": choices.auto_deskew,
        "orientation": choices.orientation,
        "photometric": choices.photometric,
        "invert": choices.invert,
        "mirror": choices.mirror,
        "lamp": choices.lamp,
    })
}

fn area_json(area: Option<(u32, u32)>) -> Value {
    area.map(|(width, height)| json!({ "width": width, "height": height })).unwrap_or(Value::Null)
}

fn span_json(span: Option<scan::ValueSpan>) -> Value {
    span.map(|span| json!({ "min": span.min, "max": span.max, "step": span.step })).unwrap_or(Value::Null)
}

/// What `scan check` was asked. Nothing is opened unless `details` or `device` says so, and nothing
/// moves unless `acquire` is set.
#[derive(Clone, Debug, Default)]
pub struct ScanCheckOptions {
    /// Open the matching device (id, name or port text) and read its capabilities. Exactly one must match.
    pub device: Option<String>,
    /// Open every device.
    pub details: bool,
    /// Make one low-resolution acquire from `device`. The scanner moves.
    pub acquire: bool,
    /// Keep the acquired pixels in this folder. Without it the pixels are dropped after counting.
    pub out: Option<std::path::PathBuf>,
}

/// The report of a check, and whether it found nothing wrong (the CLI exits non-zero when not).
#[derive(Clone, Debug)]
pub struct ScanCheck {
    pub report: Value,
    pub ok: bool,
}

/// The resolution a check acquire asks for.
pub const CHECK_ACQUIRE_DPI: u32 = 100;

/// The line shown just before a check acquire moves the scanner.
pub const CHECK_ACQUIRE_WARNING: &str = "scan check --acquire makes the scanner move: it will scan one page at 100 dpi";

type AcquireScanner<'a> = dyn FnMut(&scan::ScanSettings) -> std::result::Result<scan::Acquired, scan::ScanError> + 'a;

/// Report what the scan stack sees (`pdfcraft-cli scan check`), through the real backend. `notice` receives [`CHECK_ACQUIRE_WARNING`] right before an acquire.
pub fn scan_check(options: &ScanCheckOptions, notice: &mut dyn FnMut(&str)) -> Result<ScanCheck> {
    run_check(
        options,
        scan::supported(),
        &mut scan::scanner_list,
        &mut |id| scan::scanner_details(id),
        &mut |settings| scan::acquire(settings),
        notice,
    )
}

fn run_check(
    options: &ScanCheckOptions,
    supported: bool,
    list: &mut dyn FnMut() -> std::result::Result<scan::Listing<scan::ScannerRef>, scan::ScanError>,
    open: &mut OpenScanner<'_>,
    acquire: &mut AcquireScanner<'_>,
    notice: &mut dyn FnMut(&str),
) -> Result<ScanCheck> {
    // Guards first: they hold on every platform and before any device is touched.
    if options.acquire && options.device.is_none() {
        return Err(ToolError::InvalidArgs("--acquire needs --device ID: it makes the scanner move, so it never picks a device by itself".into()));
    }
    if options.out.is_some() && !options.acquire {
        return Err(ToolError::InvalidArgs("--out only applies together with --acquire".into()));
    }
    check_filter(options.device.as_deref())?;
    if !supported {
        return Err(failed(scan::ScanError::Unsupported.to_string()));
    }
    let (listed, backend_errors) = refs_from(list().map_err(failed)?)?;
    let mut problems: Vec<String> = Vec::new();
    for failure in &backend_errors {
        problems.push(format!("{}: {}", failure["backend"].as_str().unwrap_or("?"), failure["error"].as_str().unwrap_or("it did not list")));
    }
    let mut opened: Option<scan::Scanner> = None;
    let rows: Vec<Value> = if let Some(text) = options.device.as_deref() {
        let exact: Vec<&scan::ScannerRef> = listed.iter().filter(|item| item.id == text).collect();
        let matched: Vec<&scan::ScannerRef> = if exact.len() == 1 { exact } else { listed.iter().filter(|item| item.matches(text)).collect() };
        let item = match matched.as_slice() {
            [] => return Err(failed(format!("no scanner matches {text:?} (pdfcraft-cli scan check lists them)"))),
            [item] => *item,
            many => {
                let ids: Vec<&str> = many.iter().map(|item| item.id.as_str()).collect();
                return Err(failed(format!("{text:?} matches {} scanners; pass one id: {}", many.len(), ids.join(", "))));
            }
        };
        let (row, scanner) = opened_pair(item, open);
        opened = scanner;
        vec![row]
    } else {
        listed
            .iter()
            .map(|item| {
                if !options.details {
                    ref_row(item, scan::ListDetails::Skipped)
                } else {
                    opened_row(item, open)
                }
            })
            .collect()
    };
    for row in &rows {
        if row["details"] == scan::ListDetails::Error.as_str() {
            let name = row["name"].as_str().unwrap_or("?");
            let reason = row["error"].as_str().unwrap_or("it did not open");
            problems.push(format!("{name}: {reason}"));
        }
    }
    let mut acquire_report = Value::Null;
    if options.acquire {
        acquire_report = match &opened {
            None => {
                problems.push("acquire skipped: the device did not open".into());
                json!({ "ok": false, "error": "the device did not open" })
            }
            Some(scanner) => check_acquire(scanner, options.out.as_deref(), acquire, notice, &mut problems),
        };
    }
    let mode = if options.device.is_some() {
        "device"
    } else if options.details {
        "details"
    } else {
        "list"
    };
    let ok = problems.is_empty();
    let mut report = json!({
        "supported": true,
        "mode": mode,
        "count": rows.len(),
        "scanners": rows,
        "acquire": acquire_report,
        "ok": ok,
        "problems": problems,
    });
    if !backend_errors.is_empty() {
        report["errors"] = Value::Array(backend_errors);
    }
    Ok(ScanCheck { report, ok })
}

/// One 100 dpi grey page from `scanner`: the glass when it has one (so no paper is pulled), else the feeder.
fn check_acquire(
    scanner: &scan::Scanner,
    out: Option<&Path>,
    acquire: &mut AcquireScanner<'_>,
    notice: &mut dyn FnMut(&str),
    problems: &mut Vec<String>,
) -> Value {
    let source = if scanner.flatbed {
        scan::Source::Glass
    } else if scanner.feeder {
        scan::Source::Feeder
    } else {
        scan::Source::Auto
    };
    let choices = if source == scan::Source::Feeder { scanner.feeder_choices.as_ref() } else { scanner.glass.as_ref() };
    let dpi = choices.and_then(|choices| choices.dpi.iter().copied().filter(|dpi| *dpi >= CHECK_ACQUIRE_DPI).min()).unwrap_or(CHECK_ACQUIRE_DPI);
    let settings =
        scan::ScanSettings { device: scanner.id.clone(), source, dpi, color: scan::Color::Gray, pages: 1, ..scan::ScanSettings::default() };
    if let Err(error) = settings.checked() {
        problems.push(format!("acquire: {error}"));
        return json!({ "ok": false, "error": error.to_string() });
    }
    notice(CHECK_ACQUIRE_WARNING);
    match acquire(&settings) {
        Ok(acquired) => {
            let paths: Vec<Value> = match out {
                Some(folder) => match scan::write_pages(folder, &acquired.pages) {
                    Ok(paths) => paths.iter().map(|path| Value::String(path.display().to_string())).collect(),
                    Err(error) => {
                        problems.push(format!("acquire: could not save the pixels: {error}"));
                        Vec::new()
                    }
                },
                None => Vec::new(),
            };
            let pages: Vec<Value> = acquired
                .pages
                .iter()
                .enumerate()
                .map(|(index, page)| {
                    json!({
                            "format": page.format,
                            "width": page.width,
                            "height": page.height,
                            "dpi": page.dpi,
                    "dpi_y": page.dpi_y,
                    "requested_dpi": page.requested_dpi,
                            "bytes": page.bytes.len(),
                            "length": scan_length_json(&page.length),
                            "path": paths.get(index).cloned().unwrap_or(Value::Null),
                        })
                })
                .collect();
            if pages.is_empty() {
                problems.push("acquire: the scanner returned no page".into());
            }
            json!({
                "ok": !pages.is_empty(),
                "source": acquired.source.as_str(),
                "requested_dpi": dpi,
                "count": pages.len(),
                "pages": pages,
            })
        }
        Err(error) => {
            let info = scan::ScanErrorInfo::from_acquire(&error, &settings);
            problems.push(format!("acquire: {error}"));
            json!({ "ok": false, "error": error.to_string(), "error_info": scan_error_info_json(&info) })
        }
    }
}

/// The report as plain lines for a person (`--json` prints the report itself).
pub fn render_check_text(report: &Value) -> String {
    let mut out = String::new();
    let count = report["count"].as_u64().unwrap_or(0);
    out.push_str(&format!("scanners: {count}\n"));
    for row in report["scanners"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let text = |key: &str| row[key].as_str().unwrap_or("-").to_string();
        out.push_str(&format!(
            "- {} [{}] port {}\n  id {}\n  details: {}\n",
            text("name"),
            text("connection"),
            text("port"),
            text("id"),
            text("details")
        ));
        if let Some(error) = row["error"].as_str() {
            out.push_str(&format!("  error: {error}\n"));
        }
        if row["details"] != scan::ListDetails::Loaded.as_str() {
            continue;
        }
        let yes = |key: &str| if row[key] == true { "yes" } else { "no" };
        out.push_str(&format!(
            "  sources: glass {}, feeder {}, duplex {}; feeder ready {}\n",
            yes("flatbed"),
            yes("feeder"),
            yes("duplex"),
            yes("feeder_ready")
        ));
        let conditions =
            row["conditions"].as_array().map(|list| list.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        out.push_str(&format!("  document handling: {}\n", if conditions.is_empty() { "no fault set" } else { &conditions }));
        for label in ["bed", "sheet"] {
            if let (Some(w), Some(h)) = (row[label]["width"].as_f64(), row[label]["height"].as_f64()) {
                out.push_str(&format!("  {label} size: {:.2} x {:.2} in\n", w / 1000.0, h / 1000.0));
            }
        }
        for (label, key) in [("glass", "glass"), ("feeder", "feeder")] {
            let support = &row["page_length"][key];
            if let Some(detect) = support["device_detect"].as_str() {
                let longest = support["max_length_mils"].as_f64().map(|mils| format!(", longest page {:.2} in", mils / 1000.0)).unwrap_or_default();
                out.push_str(&format!("  {label} page length: scanner detection {detect}{longest}\n"));
            }
        }
        for (label, key) in [("glass", "glass"), ("feeder", "feeder_choices")] {
            let choices = &row[key];
            if choices.is_null() {
                continue;
            }
            let list = |name: &str| {
                let items = choices[name]
                    .as_array()
                    .map(|list| list.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string)).collect::<Vec<_>>());
                match items {
                    Some(items) if !items.is_empty() => items.join(","),
                    _ => "-".to_string(),
                }
            };
            out.push_str(&format!("  {label}: dpi {}; colors {}; formats {}\n", list("dpi"), list("colors"), list("formats")));
        }
    }
    if !report["acquire"].is_null() {
        let acquire = &report["acquire"];
        if acquire["ok"] == true {
            out.push_str(&format!("acquire: ok, {} page(s) from the {}\n", acquire["count"], acquire["source"].as_str().unwrap_or("?")));
            for page in acquire["pages"].as_array().map(Vec::as_slice).unwrap_or_default() {
                out.push_str(&format!(
                    "  {} {}x{} at {} dpi, {} bytes{}\n",
                    page["format"].as_str().unwrap_or("?"),
                    page["width"],
                    page["height"],
                    page["dpi"],
                    page["bytes"],
                    page["path"].as_str().map(|path| format!(", saved to {path}")).unwrap_or_default()
                ));
            }
        } else {
            out.push_str(&format!("acquire: failed: {}\n", acquire["error"].as_str().unwrap_or("?")));
        }
    }
    let problems: Vec<&str> = report["problems"].as_array().map(|list| list.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if problems.is_empty() {
        out.push_str("result: ok\n");
    } else {
        out.push_str(&format!("result: {} problem(s)\n", problems.len()));
        for problem in problems {
            out.push_str(&format!("  - {problem}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs on every platform: the unsupported answer (what non-Windows hosts give) is
    /// one clear sentence, as a tool error that carries the error data.
    #[test]
    fn the_unsupported_backend_reports_windows_only_for_listing_and_acquire() {
        let sentence = "scanning is only supported on Windows (WIA) in this version";
        let listed = build_listing(None, false, &[], &mut |_| Err(scan::ScanError::Unsupported));
        assert!(listed.is_ok(), "an empty list is fine");
        assert_eq!(scan::ScanError::Unsupported.to_string(), sentence);
        let folder = std::env::temp_dir();
        let settings = scan::ScanSettings::default();
        let error = scan::ScanError::Unsupported;
        match acquire_failure(&folder, &settings, error) {
            Err(ToolError::FailedWith { message, .. }) => assert_eq!(message, sentence),
            other => panic!("expected a failed tool error, got {other:?}"),
        }
    }

    #[test]
    fn an_incomplete_scan_reports_partial_true_with_the_message_and_keeps_the_page() {
        let folder = std::env::temp_dir().join(format!("pdfcraft-scan-partial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let page = scan::Page {
            bytes: b"\x89PNG\r\n\x1a\nxx".to_vec(),
            format: "png",
            width: 2,
            height: 2,
            dpi: 200,
            dpi_y: 200,
            requested_dpi: 200,
            thresholded: false,
            length: Default::default(),
        };
        let acquired = scan::Acquired { source: scan::Source::Feeder, pages: vec![page] };
        let settings = scan::ScanSettings { pages: 3, source: scan::Source::Feeder, ..scan::ScanSettings::default() };
        let message = "Only 1 of 3 pages were scanned: the feeder reported no more paper.".to_string();
        let json = finish_acquire_json(&folder, &settings, &acquired, true, Some(message.clone())).unwrap();
        assert_eq!(json["partial"], true, "{json}");
        assert_eq!(json["count"], 1);
        assert_eq!(json["error"], message.as_str());
        let text = json["error"].as_str().unwrap();
        assert!(text.contains("feeder") && text.contains("paper"), "{text}");
        assert_eq!(json["pages"].as_array().unwrap().len(), 1);
        assert!(std::path::Path::new(json["pages"][0]["path"].as_str().unwrap()).exists());
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// A backend error in the `hresult` domain, as the Windows backend builds it.
    fn hresult_error(kind: scan::InterruptKind, code: u32, name: &'static str, message: &str) -> scan::ScanError {
        scan::ScanError::native_error(kind, scan::NativeCode::hresult(code, Some(name)), message)
    }

    fn item(id: &str, name: &str, port: Option<&str>) -> scan::ScannerRef {
        scan::ScannerRef::new(id.into(), name.into(), port.map(str::to_string))
    }

    fn fixture() -> Vec<scan::ScannerRef> {
        vec![
            item("{a}", "Fake Scanner", Some("urn:uuid:00000000-0000-4000-8000-0000000000f2/svc")),
            item("{b}", "Office Printer", Some("urn:uuid:99999999-aaaa-4bbb-8ccc-dddddddddddd/svc")),
            item("{c}", "Broken", None),
        ]
    }

    fn scanner_for(id: &str) -> scan::Scanner {
        scan::Scanner {
            id: id.into(),
            name: "x".into(),
            feeder: true,
            flatbed: true,
            duplex: false,
            feeder_ready: Some(false),
            handling_status: scan::HandlingStatus::default(),
            feeder_capacity: None,
            connection: scan::Connection::Unknown,
            port: None,
            optical_x: None,
            optical_y: None,
            pages: None,
            bed: None,
            sheet: None,
            glass: None,
            feeder_choices: None,
            reported: scan::Reported::default(),
        }
    }

    fn open_fake(opened: &mut Vec<String>, id: &str) -> std::result::Result<scan::Scanner, scan::ScanError> {
        opened.push(id.to_string());
        if id == "{c}" { Err(scan::ScanError::Failed("The device is busy.".into())) } else { Ok(scanner_for(id)) }
    }

    #[test]
    fn the_default_listing_is_cheap_and_opens_nothing() {
        let mut opened = Vec::new();
        let out = build_listing(None, false, &fixture(), &mut |id| open_fake(&mut opened, id)).unwrap();
        assert!(opened.is_empty());
        assert_eq!(out["count"], 3);
        let row = &out["scanners"][0];
        assert_eq!(row["details"], "skipped");
        assert_eq!(row["id"], "{a}");
        assert_eq!(row["connection"], "wsd");
        assert!(row.get("feeder").is_none());
    }

    #[test]
    fn a_filter_opens_only_the_matching_devices() {
        let mut opened = Vec::new();
        let out = build_listing(Some("0000f2"), false, &fixture(), &mut |id| open_fake(&mut opened, id)).unwrap();
        assert_eq!(opened, ["{a}"]);
        assert_eq!(out["count"], 1);
        assert_eq!(out["scanners"][0]["details"], "loaded");
        assert_eq!(out["scanners"][0]["feeder"], true);
    }

    #[test]
    fn an_empty_filter_is_an_error() {
        for text in ["", "  "] {
            let err = build_listing(Some(text), false, &fixture(), &mut |_| Err(scan::ScanError::Unsupported)).unwrap_err();
            assert!(matches!(err, ToolError::InvalidArgs(_)), "{err}");
        }
    }

    #[test]
    fn details_opens_everything_and_keeps_a_failed_device_as_an_error_row() {
        let mut opened = Vec::new();
        let out = build_listing(None, true, &fixture(), &mut |id| open_fake(&mut opened, id)).unwrap();
        assert_eq!(opened, ["{a}", "{b}", "{c}"]);
        assert_eq!(out["count"], 3);
        assert_eq!(out["scanners"][1]["details"], "loaded");
        let bad = &out["scanners"][2];
        assert_eq!(bad["details"], "error");
        assert_eq!(bad["error"], "The device is busy.");
        assert_eq!(bad["id"], "{c}");
    }

    #[test]
    fn page_length_and_page_size_are_arguments_and_default_to_the_driver() {
        let plain = json!({ "out": "x" });
        let settings = scan_settings_from_args(&Args(&plain)).unwrap();
        assert_eq!((settings.page_length, settings.page_size), (scan::PageLength::Driver, None));
        let asked = json!({ "out": "x", "page_length": "infer", "page_size": "legal" });
        let settings = scan_settings_from_args(&Args(&asked)).unwrap();
        assert_eq!((settings.page_length, settings.page_size), (scan::PageLength::AutoInfer, Some(scan::PageSize::Legal)));
        let bad = json!({ "out": "x", "page_length": "exact" });
        let text = scan_settings_from_args(&Args(&bad)).unwrap_err().to_string();
        assert!(text.contains("page_length") && text.contains("approximate"), "{text}");
        let bad = json!({ "out": "x", "page_size": "tabloid" });
        assert!(scan_settings_from_args(&Args(&bad)).unwrap_err().to_string().contains("page_size"));
        let def = crate::tools::find("scan_acquire").unwrap();
        let lengths = def.input_schema["properties"]["page_length"]["enum"].clone();
        assert_eq!(lengths, json!(["driver", "size", "auto", "infer"]));
    }

    #[test]
    fn a_pages_length_record_says_how_it_was_obtained() {
        let driver = scan_length_json(&scan::LengthRecord::from_delivered(2800, 200));
        assert_eq!(driver, json!({ "source": "driver", "length_mils": 14000, "length_inches": 14.0, "length_mm": 355.6, "approximate": false }));
        let inferred = scan_length_json(&scan::LengthRecord::with_source(2146, 200, scan::LengthSource::Inferred));
        assert_eq!(
            (inferred["source"].clone(), inferred["approximate"].clone(), inferred["length_inches"].clone()),
            (json!("inferred"), json!(true), json!(10.73))
        );
        let unknown = scan_length_json(&scan::LengthRecord::default());
        assert_eq!((unknown["length_mils"].clone(), unknown["source"].clone()), (Value::Null, json!("driver")));
    }

    #[test]
    fn the_scanner_row_states_length_detection_three_ways() {
        let mut scanner = scanner_for("{a}");
        let unknown = scanner_json(&scanner);
        assert_eq!(unknown["page_length"]["feeder"]["device_detect"], "unknown");
        assert_eq!(unknown["page_length"]["glass"]["max_length_mils"], Value::Null);
        scanner.reported.feeder_length = scan::PageLengthSupport { device_detect: scan::Presence::Absent, max_length_mils: Some(14_000) };
        scanner.reported.glass_length = scan::PageLengthSupport { device_detect: scan::Presence::Present, max_length_mils: None };
        let row = scanner_json(&scanner);
        assert_eq!(row["page_length"]["feeder"], json!({ "device_detect": "absent", "max_length_mils": 14000 }));
        assert_eq!(row["page_length"]["glass"]["device_detect"], "present");
        let text = render_check_text(&json!({ "mode": "details", "count": 1, "scanners": [{
            "id": "{a}", "name": "x", "port": null, "connection": "unknown", "details": "loaded",
            "flatbed": true, "feeder": true, "duplex": false, "feeder_ready": false, "conditions": [],
            "glass": null, "feeder_choices": null, "page_length": row["page_length"].clone() }],
            "acquire": null, "problems": [], "ok": true, "supported": true }));
        assert!(text.contains("feeder page length: scanner detection absent, longest page 14.00 in"), "{text}");
        assert!(text.contains("glass page length: scanner detection present"), "{text}");
    }

    #[test]
    fn a_loaded_row_carries_all_nine_handling_flags_and_the_abnormal_ones_by_name() {
        let mut scanner = scanner_for("{a}");
        scanner.handling_status = scan::HandlingStatus { feeder_ready: Some(true), jam: true, attention: true, ..scan::HandlingStatus::default() };
        let row = scanner_json(&scanner);
        let flags = row["handling_status"].as_object().unwrap();
        let names: Vec<&str> = flags.keys().map(String::as_str).collect();
        let mut expected: Vec<&str> = scan::HandlingStatus::FLAG_NAMES.to_vec();
        expected.sort_unstable();
        let mut got = names.clone();
        got.sort_unstable();
        assert_eq!(got, expected);
        assert_eq!(flags["jam"], true);
        assert_eq!(flags["lamp_error"], false);
        assert_eq!(row["conditions"], json!(["jam", "attention"]));
    }

    #[test]
    fn the_rows_and_the_schema_use_the_contract_lists() {
        for details in scan::ListDetails::ALL {
            assert_eq!(details.as_str().parse::<scan::ListDetails>().ok(), Some(*details));
        }
        let rows = build_listing(None, true, &fixture(), &mut |id| open_fake(&mut Vec::new(), id)).unwrap();
        let seen: Vec<&str> = rows["scanners"].as_array().unwrap().iter().filter_map(|row| row["details"].as_str()).collect();
        assert_eq!(seen, ["loaded", "loaded", "error"]);
        let def = crate::tools::find("scan_acquire").unwrap();
        let property = |name: &str| -> Vec<String> {
            def.input_schema["properties"][name]["enum"].as_array().unwrap().iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
        };
        assert_eq!(property("source"), scan::Source::ALL.iter().map(|v| v.as_str().to_string()).collect::<Vec<_>>());
        assert_eq!(property("color"), scan::Color::ALL.iter().map(|v| v.as_str().to_string()).collect::<Vec<_>>());
    }

    #[test]
    fn a_backend_that_failed_is_reported_beside_the_devices_that_were_found() {
        let failure = scan::BackendError { backend: "wia", error: scan::ScanError::Failed("The scanner is offline.".into()) };
        let (refs, errors) = refs_from(scan::Listing { items: fixture(), errors: vec![failure] }).unwrap();
        assert_eq!(refs.len(), 3);
        assert_eq!(errors, vec![json!({ "backend": "wia", "error": "The scanner is offline." })]);
        // Nothing found and a backend failed: the failure is the answer.
        let failure = scan::BackendError { backend: "wia", error: scan::ScanError::Failed("The scanner is offline.".into()) };
        let Err(ToolError::Failed(message)) = refs_from(scan::Listing { items: Vec::new(), errors: vec![failure] }) else {
            panic!("a listing that found nothing and failed is an error");
        };
        assert_eq!(message, "The scanner is offline.");
        // Nothing found and nothing failed is an empty list.
        assert!(refs_from(scan::Listing { items: Vec::<scan::ScannerRef>::new(), errors: Vec::new() }).unwrap().0.is_empty());
    }

    #[test]
    fn a_failed_acquire_reports_its_identity_as_data() {
        let folder = std::env::temp_dir().join(format!("pdfcraft-scan-failure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let settings = scan::ScanSettings { pages: 3, source: scan::Source::Feeder, ..scan::ScanSettings::default() };
        let busy = hresult_error(scan::InterruptKind::Busy, 0x8021_0006, "WIA_ERROR_BUSY", "The scanner is busy. Try again in a moment.");
        let Err(ToolError::FailedWith { message, data }) = acquire_failure(&folder, &settings, busy) else {
            panic!("a failure with no pages is a tool error");
        };
        assert!(message.contains("busy"), "{message}");
        assert_eq!(data["kind"], "busy");
        assert_eq!(data["phase"], "before_first_page");
        assert_eq!(data["hresult"], "0x80210006");
        assert_eq!(data["code_name"], "WIA_ERROR_BUSY");
        assert_eq!((data["retryable"].clone(), data["resumable"].clone()), (json!(true), json!(false)));
        let page = scan::Page {
            bytes: b"\x89PNG\r\n\x1a\nxx".to_vec(),
            format: "png",
            width: 2,
            height: 2,
            dpi: 200,
            dpi_y: 200,
            requested_dpi: 200,
            thresholded: false,
            length: Default::default(),
        };
        let jam = scan::ScanError::Incomplete {
            message: "The scanner has a paper jam.".into(),
            acquired: scan::Acquired { source: scan::Source::Feeder, pages: vec![page] },
            cause: Some(scan::Cause {
                kind: scan::InterruptKind::Jam,
                native: Some(scan::NativeCode::hresult(0x8021_0002, Some("WIA_ERROR_PAPER_JAM"))),
            }),
        };
        let out = acquire_failure(&folder, &settings, jam).unwrap();
        assert_eq!(out["partial"], true);
        assert_eq!(out["error_info"]["kind"], "jam");
        assert_eq!(out["error_info"]["phase"], "between_pages");
        assert_eq!(out["error_info"]["code_name"], "WIA_ERROR_PAPER_JAM");
        assert_eq!((out["error_info"]["retryable"].clone(), out["error_info"]["resumable"].clone()), (json!(false), json!(true)));
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn detail_by_exact_id_returns_one_row_and_refuses_an_unknown_id() {
        let mut opened = Vec::new();
        let row = detail_row_for("{a}", &fixture(), &mut |id| open_fake(&mut opened, id)).unwrap();
        assert_eq!(row["details"], "loaded");
        let row = detail_row_for("{c}", &fixture(), &mut |id| open_fake(&mut opened, id)).unwrap();
        assert_eq!(row["details"], "error");
        assert!(detail_row_for("{zz}", &fixture(), &mut |id| open_fake(&mut opened, id)).is_err());
        assert_eq!(opened, ["{a}", "{c}"]);
    }

    // ---- scan check ----

    fn check_with(
        options: &ScanCheckOptions,
        supported: bool,
        log: &mut Vec<String>,
        notices: &mut Vec<String>,
        acquire_result: std::result::Result<scan::Acquired, scan::ScanError>,
    ) -> Result<ScanCheck> {
        let mut result = Some(acquire_result);
        let cell = std::cell::RefCell::new(log);
        run_check(
            options,
            supported,
            &mut || {
                cell.borrow_mut().push("list".into());
                Ok(scan::Listing { items: fixture(), errors: Vec::new() })
            },
            &mut |id| {
                cell.borrow_mut().push(format!("open {id}"));
                if id == "{c}" { Err(scan::ScanError::Failed("The device is busy.".into())) } else { Ok(scanner_for(id)) }
            },
            &mut |settings| {
                cell.borrow_mut().push(format!("acquire {} {} {}", settings.device, settings.dpi, settings.pages));
                result.take().unwrap_or(Err(scan::ScanError::Failed("second acquire".into())))
            },
            &mut |text| notices.push(text.to_string()),
        )
    }

    #[test]
    fn a_backend_that_failed_to_list_is_a_problem_of_the_check() {
        let check = run_check(
            &ScanCheckOptions::default(),
            true,
            &mut || {
                let failure = scan::BackendError { backend: "wia", error: scan::ScanError::Failed("The scanner is offline.".into()) };
                Ok(scan::Listing { items: fixture(), errors: vec![failure] })
            },
            &mut |id| open_fake(&mut Vec::new(), id),
            &mut |_| Err(scan::ScanError::Failed("no acquire".into())),
            &mut |_| {},
        )
        .unwrap();
        assert!(!check.ok, "{}", check.report);
        assert_eq!(check.report["problems"], json!(["wia: The scanner is offline."]));
        assert_eq!(check.report["errors"], json!([{ "backend": "wia", "error": "The scanner is offline." }]));
        assert_eq!(check.report["count"], 3);
    }

    fn one_page() -> scan::Acquired {
        let page = scan::Page {
            bytes: b"BM0123456789".to_vec(),
            format: "bmp",
            width: 85,
            height: 117,
            dpi: 100,
            dpi_y: 100,
            requested_dpi: 100,
            thresholded: false,
            length: Default::default(),
        };
        scan::Acquired { source: scan::Source::Glass, pages: vec![page] }
    }

    fn run(options: &ScanCheckOptions) -> (Result<ScanCheck>, Vec<String>, Vec<String>) {
        let (mut log, mut notices) = (Vec::new(), Vec::new());
        let result = check_with(options, true, &mut log, &mut notices, Ok(one_page()));
        (result, log, notices)
    }

    fn sorted_keys(value: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn check_lists_cheaply_and_opens_nothing_by_default() {
        let (result, log, notices) = run(&ScanCheckOptions::default());
        let check = result.unwrap();
        assert_eq!(log, ["list"]);
        assert!(notices.is_empty());
        assert!(check.ok);
        assert_eq!(check.report["mode"], "list");
        assert_eq!(check.report["count"], 3);
        assert_eq!(check.report["scanners"][0]["details"], "skipped");
        assert!(check.report["acquire"].is_null());
        let text = render_check_text(&check.report);
        assert!(text.contains("scanners: 3") && text.contains("Fake Scanner") && text.contains("result: ok"), "{text}");
    }

    #[test]
    fn check_details_opens_every_device_and_a_failed_open_is_a_problem() {
        let options = ScanCheckOptions { details: true, ..ScanCheckOptions::default() };
        let (result, log, _) = run(&options);
        let check = result.unwrap();
        assert_eq!(log, ["list", "open {a}", "open {b}", "open {c}"]);
        assert!(!check.ok, "a device that will not open fails the check");
        assert_eq!(check.report["mode"], "details");
        assert_eq!(check.report["scanners"][2]["details"], "error");
        assert_eq!(check.report["problems"], json!(["Broken: The device is busy."]));
        let text = render_check_text(&check.report);
        assert!(text.contains("error: The device is busy.") && text.contains("1 problem(s)"), "{text}");
    }

    #[test]
    fn check_device_opens_exactly_one_and_reports_its_capabilities() {
        let options = ScanCheckOptions { device: Some("0000f2".into()), ..ScanCheckOptions::default() };
        let (result, log, _) = run(&options);
        let check = result.unwrap();
        assert_eq!(log, ["list", "open {a}"]);
        assert!(check.ok);
        assert_eq!(check.report["mode"], "device");
        assert_eq!(check.report["scanners"][0]["details"], "loaded");
        let text = render_check_text(&check.report);
        assert!(text.contains("sources: glass yes, feeder yes, duplex no") && text.contains("document handling: no fault set"), "{text}");
    }

    #[test]
    fn check_device_refuses_no_match_and_many_matches() {
        let (mut log, mut notices) = (Vec::new(), Vec::new());
        for (text, wanted) in [("nothing like it", "no scanner matches"), ("e", "matches 3 scanners")] {
            let options = ScanCheckOptions { device: Some(text.into()), ..ScanCheckOptions::default() };
            let error = check_with(&options, true, &mut log, &mut notices, Ok(one_page())).unwrap_err();
            assert!(matches!(&error, ToolError::Failed(message) if message.contains(wanted)), "{text}: {error:?}");
        }
    }

    #[test]
    fn check_json_schema_is_stable() {
        let options = ScanCheckOptions { details: true, ..ScanCheckOptions::default() };
        let (result, _, _) = run(&options);
        let report = result.unwrap().report;
        assert_eq!(sorted_keys(&report), ["acquire", "count", "mode", "ok", "problems", "scanners", "supported"]);
        assert_eq!(
            sorted_keys(&report["scanners"][0]),
            [
                "bed",
                "conditions",
                "connection",
                "details",
                "duplex",
                "feeder",
                "feeder_capacity",
                "feeder_choices",
                "feeder_ready",
                "flatbed",
                "glass",
                "handling_status",
                "id",
                "name",
                "optical_x",
                "optical_y",
                "page_length",
                "pages",
                "port",
                "sheet"
            ]
        );
        assert_eq!(report["scanners"][0]["handling_status"].as_object().unwrap().len(), scan::HandlingStatus::FLAG_NAMES.len());
        let options = ScanCheckOptions { device: Some("{a}".into()), acquire: true, ..ScanCheckOptions::default() };
        let (result, _, _) = run(&options);
        let acquire = result.unwrap().report["acquire"].clone();
        assert_eq!(sorted_keys(&acquire), ["count", "ok", "pages", "requested_dpi", "source"]);
        assert_eq!(sorted_keys(&acquire["pages"][0]), ["bytes", "dpi", "dpi_y", "format", "height", "length", "path", "requested_dpi", "width"]);
    }

    #[test]
    fn check_acquire_needs_a_device_and_touches_nothing_without_one() {
        let (mut log, mut notices) = (Vec::new(), Vec::new());
        for options in [
            ScanCheckOptions { acquire: true, ..ScanCheckOptions::default() },
            ScanCheckOptions { acquire: true, details: true, ..ScanCheckOptions::default() },
        ] {
            let error = check_with(&options, true, &mut log, &mut notices, Ok(one_page())).unwrap_err();
            assert!(matches!(&error, ToolError::InvalidArgs(message) if message.contains("--acquire needs --device")), "{error:?}");
        }
        let options = ScanCheckOptions { out: Some(std::env::temp_dir()), ..ScanCheckOptions::default() };
        assert!(matches!(check_with(&options, true, &mut log, &mut notices, Ok(one_page())), Err(ToolError::InvalidArgs(_))));
        assert!(log.is_empty() && notices.is_empty(), "no list, open or acquire happened: {log:?}");
    }

    #[test]
    fn check_acquire_warns_then_scans_one_page_at_100_dpi_and_reports_only_facts() {
        let options = ScanCheckOptions { device: Some("{a}".into()), acquire: true, ..ScanCheckOptions::default() };
        let (result, log, notices) = run(&options);
        let check = result.unwrap();
        assert_eq!(log, ["list", "open {a}", "acquire {a} 100 1"]);
        assert_eq!(notices, [CHECK_ACQUIRE_WARNING]);
        assert!(CHECK_ACQUIRE_WARNING.contains("makes the scanner move") && !CHECK_ACQUIRE_WARNING.contains('\n'));
        assert!(check.ok);
        let acquire = &check.report["acquire"];
        assert_eq!((acquire["ok"].clone(), acquire["count"].clone(), acquire["source"].clone()), (json!(true), json!(1), json!("glass")));
        assert_eq!(acquire["pages"][0]["bytes"], 12);
        assert_eq!(acquire["pages"][0]["format"], "bmp");
        assert!(acquire["pages"][0]["path"].is_null(), "pixels are not written without --out");
        assert!(render_check_text(&check.report).contains("bmp 85x117 at 100 dpi, 12 bytes"));
    }

    #[test]
    fn check_acquire_writes_pixels_only_into_out() {
        let folder = std::env::temp_dir().join(format!("pdfcraft-scan-check-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let options = ScanCheckOptions { device: Some("{a}".into()), acquire: true, out: Some(folder.clone()), ..ScanCheckOptions::default() };
        let (result, _, _) = run(&options);
        let check = result.unwrap();
        let path = check.report["acquire"]["pages"][0]["path"].as_str().unwrap().to_string();
        assert_eq!(std::fs::read(&path).unwrap(), b"BM0123456789");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn check_acquire_failure_and_unopenable_device_are_problems_with_data() {
        let (mut log, mut notices) = (Vec::new(), Vec::new());
        let options = ScanCheckOptions { device: Some("{a}".into()), acquire: true, ..ScanCheckOptions::default() };
        let busy = hresult_error(scan::InterruptKind::Busy, 0x8021_0006, "WIA_ERROR_BUSY", "The scanner is busy. Try again in a moment.");
        let check = check_with(&options, true, &mut log, &mut notices, Err(busy)).unwrap();
        assert!(!check.ok);
        assert_eq!(check.report["acquire"]["ok"], false);
        assert_eq!(check.report["acquire"]["error_info"]["kind"], "busy");
        let options = ScanCheckOptions { device: Some("{c}".into()), acquire: true, ..ScanCheckOptions::default() };
        let (mut log, mut notices) = (Vec::new(), Vec::new());
        let check = check_with(&options, true, &mut log, &mut notices, Ok(one_page())).unwrap();
        assert!(!check.ok);
        assert!(log.iter().all(|line| !line.starts_with("acquire")), "{log:?}");
        assert!(notices.is_empty(), "the warning is only for a scan that really starts");
    }

    #[test]
    fn check_without_windows_says_so_after_the_guards_and_touches_nothing() {
        let (mut log, mut notices) = (Vec::new(), Vec::new());
        let error = check_with(&ScanCheckOptions::default(), false, &mut log, &mut notices, Ok(one_page())).unwrap_err();
        assert!(matches!(&error, ToolError::Failed(m) if m == "scanning is only supported on Windows (WIA) in this version"), "{error:?}");
        let options = ScanCheckOptions { acquire: true, ..ScanCheckOptions::default() };
        let error = check_with(&options, false, &mut log, &mut notices, Ok(one_page())).unwrap_err();
        assert!(matches!(error, ToolError::InvalidArgs(_)), "the guard answers the same on every platform");
        assert!(log.is_empty() && notices.is_empty());
    }
}
