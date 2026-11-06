use super::commands::{parse_dpi, read};
use super::options::{AUDIT_OPTIONS, CHECK_ONE_OPTIONS, CHECK_OPTIONS, flag, positional, validate_options};
use super::{
    Arc, CliError, Command, Duration, Instant, PageRenderer, Path, PathBuf, RenderConfig, RenderRequest, RequestKind, Stdio, inspect, stdout_line,
};

/// Audit a document for faux redactions. Prints the findings as JSON; fails when any are
/// found so scripts and CI can gate on it, and when the audit itself cannot run.
pub(crate) fn audit(args: &[String]) -> Result<(), CliError> {
    validate_options("audit", args, AUDIT_OPTIONS)?;
    let path = *positional(args, AUDIT_OPTIONS).first().ok_or("audit: missing file")?;
    let bytes = read(path)?;
    let password = flag(args, "--password");
    let doc = pdfcraft_cos::Document::open_with_password(bytes.clone(), password).map_err(|e| format!("{path}: {e}"))?;
    let findings = pdfcraft_audit::audit_redactions(&doc).map_err(|e| format!("{path}: {e}"))?;
    // Rectangles in points from the top-left of the displayed page, as `doc_audit_redactions`
    // reports them; the page geometry is read only when there is something to place.
    let info = if findings.is_empty() {
        None
    } else {
        let render_password = pdfcraft_engine::render_password_for(bytes.clone(), password);
        Some(inspect(bytes, render_password.as_deref()).map_err(|e| format!("{path}: {e}"))?)
    };
    let items: Vec<serde_json::Value> = findings
        .into_iter()
        .map(|f| {
            let rect = f.rect.map(|v| v as f32);
            serde_json::json!({
                "page": f.page + 1,
                "kind": f.kind.id(),
                "rect": info.as_ref().map_or(rect, |i| pdfcraft_automation::view_rect(i, f.page, rect)),
                "covered_text": f.covered_text,
            })
        })
        .collect();
    println!("{}", serde_json::json!({ "file": path, "findings": items, "count": items.len() }));
    if items.is_empty() { Ok(()) } else { Err(format!("audit: {} faux redaction(s) found", items.len()).into()) }
}

/// Child-process body for `check`: prints one JSON line.
pub(crate) fn check_one(args: &[String]) -> Result<(), CliError> {
    validate_options("check-one", args, CHECK_ONE_OPTIONS)?;
    let path = *positional(args, CHECK_ONE_OPTIONS).first().ok_or("check-one: missing file")?;
    let dpi = parse_dpi(flag(args, "--dpi").unwrap_or("36"))?;
    let start = Instant::now();
    let bytes = read(path)?;
    let (status, pages, failed, detail, warnings) = match inspect(bytes.clone(), None) {
        Err(e) => (format!("open-error:{}", short(&e.to_string())), 0, 0, e.to_string(), 0),
        // A dynamic XFA form is laid out at open (#1645): like `info`, `text` and `render`, check
        // the copy the app shows — the laid-out page tree, `session.display` — not the
        // placeholder page the file still holds.
        Ok(info) if info.xfa == Some(pdfcraft_render::Xfa::Dynamic) => {
            let mut session = pdfcraft_engine::Session::new();
            match session.open(path, None, bytes, None) {
                Err(e) => (format!("open-error:{}", short(&e.to_string())), 0, 0, e.to_string(), 0),
                Ok(id) => match session.get(id) {
                    // Open succeeds even when the layout fails: the placeholder page stays and the
                    // reason lands in the warnings. The placeholder renders, so reporting it as
                    // `ok` would say a broken form checks fine — the layout failure is the finding.
                    Some(doc) if doc.xfa.is_none() => {
                        let detail = doc
                            .info
                            .warnings
                            .iter()
                            .find(|w| w.starts_with("This dynamic XFA form"))
                            .cloned()
                            .unwrap_or_else(|| "the dynamic XFA form could not be laid out".to_string());
                        ("xfa-layout-failed".to_string(), doc.info.pages.len(), 1, detail, doc.info.warnings.len())
                    }
                    // A successful layout is what the app shows: its pages render, and its own
                    // warnings live in the layout report, next to the file inspection's.
                    Some(doc) => {
                        let warnings = doc.info.warnings.len() + doc.xfa.as_ref().map_or(0, |xfa| xfa.warnings.len());
                        check_pages(doc.display.clone(), doc.info.pages.len(), warnings, dpi)
                    }
                    None => {
                        let e = "the document could not be opened".to_string();
                        (format!("open-error:{}", short(&e)), 0, 0, e, 0)
                    }
                },
            }
        }
        Ok(info) => check_pages(bytes, info.pages.len(), info.warnings.len(), dpi),
    };
    // `--edit` (fuzzing): also run our own object layer end to end, with no panic safety net,
    // so a panic in cos shows up as a crash of this process.
    if args.iter().any(|a| a == "--edit") {
        edit_round_trip(&std::fs::read(path).map_err(|e| e.to_string())?);
    }
    let line = serde_json::json!({ "file": path, "status": status, "pages": pages, "failed_pages": failed, "warnings": warnings,
        "ms": start.elapsed().as_millis() as u64, "detail": detail.chars().take(400).collect::<String>() });
    stdout_line(format_args!("{line}"))?;
    Ok(())
}

/// Render each page of `bytes` up to `count` (pixels at `--dpi` and text at 1.0) and give the
/// status, page count, failure count, detail and warning count `check` reports.
fn check_pages(bytes: Arc<Vec<u8>>, count: usize, warnings: usize, dpi: f32) -> (String, usize, usize, String, usize) {
    let mut r = PageRenderer::new(bytes, RenderConfig::default());
    let mut failed = Vec::new();
    for i in 0..count.min(500) {
        let p = r.render(RenderRequest { page: i, kind: RequestKind::Pixels, tile: None, scale: dpi / 72.0, tag: 0, preview: false });
        if let Some(e) = r.render(RenderRequest { page: i, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, preview: false }).error {
            failed.push(format!("p{} text: {e}", i + 1));
        }
        if let Some(e) = p.error {
            failed.push(format!("p{}: {e}", i + 1));
        }
    }
    let status = if failed.is_empty() { "ok".to_string() } else { "page-errors".to_string() };
    (status, count, failed.len(), failed.join(" | "), warnings)
}

/// Open with cos, touch every object, edit the catalog, save incrementally and in full (both
/// output styles), and reopen each result. Errors are fine; panics, hangs and aborts are bugs.
fn edit_round_trip(bytes: &[u8]) {
    use pdfcraft_cos::{Document, Object, SaveOptions, write_full, write_incremental};
    let Ok(mut doc) = Document::open(Arc::new(bytes.to_vec())) else { return };
    for num in doc.object_numbers().into_iter().take(20_000) {
        if let Ok(o) = doc.try_get(num)
            && let Object::Stream(s) = &*o
        {
            let _ = s.decoded();
        }
    }
    if let Some(root) = doc.root() {
        let _ = doc.update_dict(root, |d| d.set(b"PdfCraftFuzz".to_vec(), Object::Bool(true)));
    }
    let classic = SaveOptions { object_streams: false, ..SaveOptions::default() };
    for out in [write_incremental(&doc, &SaveOptions::default()), write_full(&doc, &SaveOptions::default()), write_full(&doc, &classic)]
        .into_iter()
        .flatten()
    {
        if let Ok(again) = Document::open(Arc::new(out)) {
            let _ = again.root().map(|r| again.get(r));
        }
    }
}

fn short(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).take(4).collect::<Vec<_>>().join("-").to_lowercase()
}

fn collect(paths: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let mut visited = std::collections::HashSet::new();
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            // Directory aliases and symlink cycles must not scan the same tree repeatedly.
            let identity = match p.canonicalize() {
                Ok(identity) => identity,
                Err(error) => {
                    eprintln!("{}: could not resolve directory: {error}", p.display());
                    continue;
                }
            };
            if !visited.insert(identity) {
                continue;
            }
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        } else if p.extension().and_then(std::ffi::OsStr::to_str).is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
            out.push(p);
        }
    }
    out.sort();
    out
}

pub(crate) fn check(args: &[String]) -> Result<(), CliError> {
    validate_options("check", args, CHECK_OPTIONS)?;
    // Every option is checked before the inputs are scanned, so a bad value is one message,
    // not one problem per file: a zero timeout used to make every file time out, and a bad
    // dpi used to reach each child, which printed nothing and was reported as a crash.
    let timeout_value = flag(args, "--timeout").unwrap_or("20");
    let timeout = Duration::from_secs(
        timeout_value
            .parse::<u64>()
            .ok()
            .filter(|secs| *secs > 0)
            .ok_or_else(|| format!("bad --timeout: expected a positive whole number of seconds, got {timeout_value:?}"))?,
    );
    let dpi = flag(args, "--dpi").unwrap_or("36").to_string();
    parse_dpi(&dpi)?;
    let files = collect(&positional(args, CHECK_OPTIONS));
    if files.is_empty() {
        return Err("check: no PDF files found".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for (n, f) in files.iter().enumerate() {
        let r = run_child(&exe, f, &dpi, timeout);
        let status = r["status"].as_str().unwrap_or("?").to_string();
        *counts.entry(status.split(':').next().unwrap_or("?").to_string()).or_default() += 1;
        if status != "ok" {
            eprintln!(
                "[{}/{}] {status:<28} {}  {}",
                n + 1,
                files.len(),
                f.display(),
                r["detail"].as_str().unwrap_or("").chars().take(160).collect::<String>()
            );
        }
        results.push(r);
    }
    eprintln!("\nchecked {} files: {:?}", files.len(), counts);
    if let Some(out) = flag(args, "--json") {
        std::fs::write(out, serde_json::to_string_pretty(&results).unwrap_or_default()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn run_child(exe: &Path, file: &Path, dpi: &str, timeout: Duration) -> serde_json::Value {
    let file_s = file.to_string_lossy().to_string();
    let child = Command::new(exe).args(["check-one", &file_s, "--dpi", dpi]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return serde_json::json!({ "file": file_s, "status": "harness-error", "detail": e.to_string() }),
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut so) = child.stdout.take() {
                    use std::io::Read;
                    let _ = so.read_to_string(&mut out);
                }
                return match serde_json::from_str::<serde_json::Value>(out.trim()) {
                    Ok(v) => v,
                    Err(_) => serde_json::json!({ "file": file_s, "status": "crash", "detail": format!("child exited with {status}") }),
                };
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return serde_json::json!({ "file": file_s, "status": "timeout", "detail": format!("exceeded {}s", timeout.as_secs()) });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return serde_json::json!({ "file": file_s, "status": "harness-error", "detail": e.to_string() }),
        }
    }
}
