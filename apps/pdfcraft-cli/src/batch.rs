//! Friendly flags for the same folder-job tool exposed by `run` and MCP.

use serde_json::json;

use std::path::{Path, PathBuf};

use crate::{CliError, OptionSpec, automation, flag, print_output, validate_options};

const OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--root", takes_value: true },
    OptionSpec { name: "--out-dir", takes_value: true },
    OptionSpec { name: "--steps", takes_value: true },
    OptionSpec { name: "--recursive", takes_value: false },
    OptionSpec { name: "--stop-on-error", takes_value: false },
    OptionSpec { name: "--ppi", takes_value: true },
    OptionSpec { name: "--quality", takes_value: true },
    OptionSpec { name: "--dpi", takes_value: true },
    OptionSpec { name: "--language", takes_value: true },
    OptionSpec { name: "--ocr-all-pages", takes_value: false },
    OptionSpec { name: "--log", takes_value: true },
    OptionSpec { name: "--dry-run", takes_value: false },
];

pub(super) fn run(args: &[String]) -> Result<(), CliError> {
    validate_options("batch", args, OPTIONS)?;
    let mut paths = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if let Some(spec) = OPTIONS.iter().find(|s| s.name == arg) {
            if !seen.insert(spec.name) {
                return Err(format!("batch: {} may only be supplied once", spec.name).into());
            }
            if spec.takes_value {
                if args.get(i + 1).is_none_or(|v| v.is_empty() || v.starts_with("--")) {
                    return Err(format!("batch: {} needs a value", spec.name).into());
                }
                i += 1;
            }
        } else {
            paths.push(arg);
        }
        i += 1;
    }
    let steps: Vec<_> = flag(args, "--steps").ok_or("batch: missing --steps (convert, ocr, optimize)")?.split(',').collect();
    let out = flag(args, "--out-dir").ok_or("batch: missing --out-dir")?;
    let mut params = json!({"paths":paths,"steps":steps,"out_dir":out,"recursive":seen.contains("--recursive"),"stop_on_error":seen.contains("--stop-on-error"),"dry_run":seen.contains("--dry-run")});
    for (option, key) in [("--ppi", "ppi"), ("--quality", "quality"), ("--dpi", "dpi")] {
        if let Some(v) = flag(args, option) {
            params[key] = serde_json::from_str(v).map_err(|_| format!("batch: {option} needs a number"))?;
        }
    }
    if let Some(lang) = flag(args, "--language") {
        params["language"] = json!(lang);
    }
    if seen.contains("--ocr-all-pages") {
        params["skip_text_pages"] = json!(false);
    }
    let mut auto = automation(args)?;
    let content = auto.call("doc_batch", &params).map_err(|e| e.to_string())?;
    let failed = content.iter().any(|c| matches!(c, pdfcraft_automation::Content::Json(v) if v["failed"].as_u64().is_some_and(|n| n > 0)));
    let mut log_error: Option<String> = None;
    if let Some(path) = flag(args, "--log") {
        let log = content.iter().find_map(|c| match c {
            pdfcraft_automation::Content::Json(v) => v["log"].as_str().map(str::to_owned),
            _ => None,
        });
        // Never let the log replace a file the batch read or published: collect every
        // discovered input and every output this batch wrote, then refuse a match.
        let protected: Vec<PathBuf> = content
            .iter()
            .filter_map(|c| match c {
                pdfcraft_automation::Content::Json(v) => v["files"].as_array().map(|files| {
                    files.iter().flat_map(|f| {
                        let mut names = Vec::new();
                        if let Some(p) = f.get("path").and_then(|v| v.as_str()) {
                            names.push(PathBuf::from(p));
                        }
                        if matches!(f.get("status").and_then(|v| v.as_str()), Some("ok") | Some("planned"))
                            && let Some(p) = f.get("output").and_then(|v| v.as_str())
                        {
                            names.push(PathBuf::from(p));
                        }
                        names
                    })
                }),
                _ => None,
            })
            .flatten()
            .collect();
        match auto.resolve_path(path, true) {
            Ok(target) => {
                if target.is_dir() {
                    log_error = Some(format!("batch: --log {path} is a directory"));
                } else if protected.iter().any(|p| same_file(&target, p)) {
                    log_error = Some(format!("batch: --log {path} would overwrite a batch input or output"));
                } else if let Err(e) = pdfcraft_platform::staging::write_atomically(
                    &target,
                    log.unwrap_or_default().as_bytes(),
                    pdfcraft_platform::staging::StagingName::SuffixThenTag,
                ) {
                    log_error = Some(format!("batch: cannot write --log {path}: {e}"));
                }
            }
            Err(e) => {
                log_error = Some(format!("batch: cannot resolve --log {path}: {e}"));
            }
        }
    }
    let print_result = print_output(&auto, content, None);
    if let Some(error) = log_error {
        return Err(error.into());
    }
    print_result?;
    if failed { Err("batch: one or more files failed; see the JSON report".into()) } else { Ok(()) }
}

/// Whether `a` and `b` name the same file, normalizing `.` and `..` components and resolving
/// existing ancestors even when the final path does not exist yet.
fn same_file(a: &Path, b: &Path) -> bool {
    normalize_path(a) == normalize_path(b)
}

/// Lexically and physically normalize a path, including paths that do not exist yet.
fn normalize_path(path: &Path) -> PathBuf {
    // Resolve symlinks in existing components before applying `..`, so an alias such as
    // `out/link/../../protected.pdf` cannot hide behind lexical path normalization.
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(resolved.components().next_back(), Some(std::path::Component::Normal(_))) {
                    resolved.pop();
                } else {
                    resolved.push(component.as_os_str());
                }
            }
            _ => {
                resolved.push(component.as_os_str());
                if let Ok(canonical) = resolved.canonicalize() {
                    resolved = canonical;
                }
            }
        }
    }
    if resolved.as_os_str().is_empty() {
        resolved = PathBuf::from(".");
    }

    if let Ok(canonical) = resolved.canonicalize() {
        return finish_path(canonical);
    }

    // Canonicalize the deepest existing ancestor, then reattach the (possibly nonexistent) tail.
    // This also resolves symlinks in the existing portion of the path.
    let mut ancestor = resolved.as_path();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !ancestor.exists() {
        match ancestor.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                match ancestor.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => ancestor = parent,
                    _ => break,
                }
            }
            None => break,
        }
    }
    let base = ancestor.canonicalize().unwrap_or_else(|_| ancestor.to_path_buf());
    let mut result = base;
    for name in tail.into_iter().rev() {
        result.push(name);
    }
    finish_path(result)
}

/// Normalize casing for platforms whose filesystems are case-insensitive. This is used only for
/// equality checks, never for opening files.
fn finish_path(path: PathBuf) -> PathBuf {
    #[cfg(any(windows, target_os = "macos", target_os = "ios"))]
    {
        if let Some(text) = path.to_str() {
            return PathBuf::from(text.to_lowercase());
        }
    }
    path
}
