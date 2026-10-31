use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind, inspect};

use crate::cli::{CheckArgs, CheckOneArgs};
use crate::commands::read;
use crate::error::CliError;
use crate::{Io, stdout_line};

/// Child-process body for `check`: prints one JSON line.
pub(crate) fn check_one(args: &CheckOneArgs, io: &mut Io) -> Result<(), CliError> {
    let path = args.file.as_path();
    let dpi = args.dpi;
    let start = Instant::now();
    let bytes = read(path)?;
    let (status, pages, failed, detail, warnings) = match inspect(bytes.clone(), None) {
        Err(e) => (format!("open-error:{}", short(&e.to_string())), 0, 0, e.to_string(), 0),
        Ok(info) => {
            let mut r = PageRenderer::new(bytes, RenderConfig::default());
            let mut failed = Vec::new();
            for i in 0..info.pages.len().min(500) {
                let p = r.render(RenderRequest { page: i, kind: RequestKind::Pixels, tile: None, scale: dpi / 72.0, tag: 0, preview: false });
                if let Some(e) = r.render(RenderRequest { page: i, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, preview: false }).error {
                    failed.push(format!("p{} text: {e}", i + 1));
                }
                if let Some(e) = p.error {
                    failed.push(format!("p{}: {e}", i + 1));
                }
            }
            let status = if failed.is_empty() { "ok".to_string() } else { "page-errors".to_string() };
            (status, info.pages.len(), failed.len(), failed.join(" | "), info.warnings.len())
        }
    };
    // `--edit` (fuzzing): also run our own object layer end to end, with no panic safety net,
    // so a panic in cos shows up as a crash of this process.
    if args.edit {
        edit_round_trip(&std::fs::read(path).map_err(|e| CliError::file(path, e))?);
    }
    let line = serde_json::json!({ "file": path.to_string_lossy(), "status": status, "pages": pages, "failed_pages": failed, "warnings": warnings,
        "ms": start.elapsed().as_millis() as u64, "detail": detail.chars().take(400).collect::<String>() });
    stdout_line(io, format_args!("{line}"))
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

fn collect(paths: &[PathBuf], io: &mut Io) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = paths.to_vec();
    let mut visited = std::collections::HashSet::new();
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            // Directory aliases and symlink cycles must not scan the same tree repeatedly.
            let identity = match p.canonicalize() {
                Ok(identity) => identity,
                Err(error) => {
                    let _ = writeln!(io.err, "{}: could not resolve directory: {error}", p.display());
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

/// Runs each file in a child process of the running executable, so a hang or crash in any
/// dependency is observed and reported instead of taking the harness down.
pub(crate) fn check(args: &CheckArgs, io: &mut Io) -> Result<(), CliError> {
    let timeout = Duration::from_secs(args.timeout);
    let files = collect(&args.paths, io);
    if files.is_empty() {
        return Err(CliError::NoPdfFiles);
    }
    let exe = std::env::current_exe().map_err(|e| CliError::command("check", e.to_string()))?;
    let mut results = Vec::new();
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for (n, f) in files.iter().enumerate() {
        let r = run_child(&exe, f, args.dpi, timeout);
        let status = r["status"].as_str().unwrap_or("?").to_string();
        *counts.entry(status.split(':').next().unwrap_or("?").to_string()).or_default() += 1;
        if status != "ok" {
            let _ = writeln!(
                io.err,
                "[{}/{}] {status:<28} {}  {}",
                n + 1,
                files.len(),
                f.display(),
                r["detail"].as_str().unwrap_or("").chars().take(160).collect::<String>()
            );
        }
        results.push(r);
    }
    let _ = writeln!(io.err, "\nchecked {} files: {:?}", files.len(), counts);
    if let Some(out) = &args.json {
        std::fs::write(out, serde_json::to_string_pretty(&results).unwrap_or_default()).map_err(|e| CliError::file(out, e))?;
    }
    Ok(())
}

fn run_child(exe: &Path, file: &Path, dpi: f32, timeout: Duration) -> serde_json::Value {
    let file_s = file.to_string_lossy().to_string();
    let child = Command::new(exe).arg("check-one").arg(file).arg("--dpi").arg(dpi.to_string()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
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
