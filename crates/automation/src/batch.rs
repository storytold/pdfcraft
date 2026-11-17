//! One-shot folder jobs shared by CLI and MCP: the arguments and `--root` confinement. Finding
//! the files, running the steps and publishing without replacing anything is
//! `pdfcraft_engine::batch`, which the desktop app uses too.

use std::collections::HashSet;
use std::path::PathBuf;

use pdfcraft_engine::batch::{self, Discovery, Recipe, SkipReason, Step, StepLog};
use pdfcraft_engine::optimize;
use serde_json::{Value, json};

use crate::{Args, Automation, Result, failed};

fn recipe(auto: &Automation, args: &Args) -> Result<Recipe> {
    let names = args.strs("steps")?;
    if names.is_empty() || names.len() > 3 {
        return Err(Args::wrong("steps", "one to three unique steps: convert, ocr, optimize"));
    }
    let mut steps = Vec::new();
    for (i, name) in names.iter().enumerate() {
        match Step::parse(name) {
            Some(step) if !steps.contains(&step) && (step != Step::Convert || i == 0) => steps.push(step),
            _ => return Err(Args::wrong("steps", "unique convert/ocr/optimize steps, with convert first when present")),
        }
    }
    for (keys, step) in [(&["ppi", "quality"][..], Step::Optimize), (&["dpi", "language", "skip_text_pages"][..], Step::Ocr)] {
        if !steps.contains(&step) && keys.iter().any(|k| args.get(k).is_some()) {
            return Err(failed(format!("{} settings need the {} step", keys.join("/"), step.name())));
        }
    }
    let mut images = optimize::ImageSettings::REDUCE;
    let mut mono = optimize::MonoSettings::REDUCE;
    if let Some(ppi) = args.opt_num("ppi")? {
        if !(9.0..=1600.0).contains(&ppi) {
            return Err(Args::wrong("ppi", "9 to 1600"));
        }
        images.target_ppi = ppi;
        images.above_ppi = ppi * 1.5;
        // Black-and-white images keep Reduce File Size's higher resolution, at which text
        // stays legible, and are never taken below a higher `ppi` asked for.
        mono.target_ppi = mono.target_ppi.max(ppi);
        mono.above_ppi = mono.target_ppi * 1.5;
    }
    if let Some(q) = args.opt_int("quality")? {
        if !(1..=100).contains(&q) {
            return Err(Args::wrong("quality", "1 to 100"));
        }
        images.compression = optimize::Compression::Jpeg(q as u8);
    }
    if args.opt_num("dpi")?.is_some_and(|dpi| !(72.0..=600.0).contains(&dpi)) {
        return Err(Args::wrong("dpi", "72 to 600"));
    }
    // The calling session's JavaScript preference, carried into each file's own session.
    Ok(Recipe { steps, images, mono, ocr: auto.ocr_settings(args)?, javascript: auto.session.javascript() })
}

fn step_json(log: &StepLog) -> Value {
    match log {
        StepLog::Ocr { words, skipped_pages, .. } => json!({"step":"ocr","words":words,"skipped_pages":skipped_pages}),
        StepLog::Size { step, before, after } => json!({"step":step.name(),"bytes_before":before,"bytes_after":after}),
    }
}

impl Automation {
    pub(crate) fn doc_batch(&self, args: &Args) -> Result<Value> {
        let recipe = recipe(self, args)?;
        let recursive = args.opt_bool("recursive")?.unwrap_or(false);
        let stop = args.opt_bool("stop_on_error")?.unwrap_or(false);
        let dry_run = args.opt_bool("dry_run")?.unwrap_or(false);
        let paths = args.strs("paths")?;
        if paths.is_empty() || paths.len() > 1000 {
            return Err(Args::wrong("paths", "1 to 1000 files or folders"));
        }
        let out = batch::absolute(&self.resolve(args.str("out_dir")?, true)?).map_err(failed)?;
        let mut roots = Vec::new();
        for path in paths {
            let resolved = self.resolve(path, false)?;
            let original = self.root.as_ref().map_or_else(|| PathBuf::from(path), |root| root.join(path));
            if batch::linked(&original.symlink_metadata().map_err(failed)?) {
                return Err(failed("input roots must be regular files or folders, not links"));
            }
            roots.push(batch::absolute(&resolved).map_err(failed)?);
        }
        let discovery = Discovery::find(&roots, &out, recursive).map_err(|e| {
            failed(if e.starts_with("the output folder") { "out_dir must not contain an input folder or equal it".to_string() } else { e })
        })?;
        let mut files: Vec<Value> =
            discovery.skipped.iter().map(|(path, why)| json!({"path":path.to_string_lossy(),"status":"skipped","reason":why.text()})).collect();
        let (mut completed, mut failed_count) = (0, 0);
        let mut stopped = false;
        let mut planned_outputs: HashSet<PathBuf> = HashSet::new();
        for (path, relative) in discovery.files {
            if stopped || !recipe.accepts(&path) {
                let why = if stopped { SkipReason::Stopped } else { SkipReason::Unsupported };
                files.push(json!({"path":path.to_string_lossy(),"status":"skipped","reason":why.text()}));
                continue;
            }
            // A dry run reports where each result would go, naming the file a real publish would
            // pick, and writes nothing.
            if dry_run {
                let planned: Result<PathBuf> = (|| {
                    let target = batch::output_path(&out, &relative);
                    // The write root gets its say on the name. Without a root, resolve hands the
                    // name back unchanged, so it is not what confinement may be checked against.
                    self.resolve(target.to_str().ok_or_else(|| failed("output path is not UTF-8"))?, true)?;
                    let target = batch::confined_output(&out, &relative).map_err(failed)?;
                    let dir = target.parent().ok_or_else(|| failed("output has no parent"))?;
                    let stem = target.file_stem().ok_or_else(|| failed("output has no stem"))?.to_string_lossy().into_owned();
                    for n in 1..=10_000 {
                        let candidate = if n == 1 { target.clone() } else { dir.join(format!("{stem} ({n}).pdf")) };
                        if candidate.symlink_metadata().is_ok() || planned_outputs.contains(&candidate) {
                            continue;
                        }
                        planned_outputs.insert(candidate.clone());
                        return Ok(candidate);
                    }
                    Err(failed("all 10000 output names are occupied"))
                })();
                match planned {
                    Ok(target) => files.push(json!({
                        "path": path.to_string_lossy(),
                        "status": "planned",
                        "output": target.to_string_lossy().replace('\\', "/"),
                        "steps": recipe.steps.iter().map(|s| s.name()).collect::<Vec<_>>(),
                    })),
                    Err(e) => {
                        failed_count += 1;
                        files.push(json!({"path":path.to_string_lossy(),"status":"error","error":e.to_string()}));
                        stopped = stop;
                    }
                }
                continue;
            }
            // Recheck confinement and links before reading a discovered file.
            let result: Result<_> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let current = self.resolve(path.to_str().ok_or_else(|| failed("input path is not UTF-8"))?, false)?;
                if batch::linked(&path.symlink_metadata().map_err(failed)?) || batch::absolute(&current).map_err(failed)? != path {
                    return Err(failed("input path changed after discovery"));
                }
                // Not a FIFO or device that would block the read.
                batch::unchanged(&path).map_err(failed)?;
                let (bytes, log) = recipe.process(&current, |_, _| true).map_err(failed)?;
                let target = batch::output_path(&out, &relative);
                // The write root gets its say on the name; the links under out are what decide
                // where the result really lands, so confinement is checked against those.
                self.resolve(target.to_str().ok_or_else(|| failed("output path is not UTF-8"))?, true)?;
                let target = batch::confined_output(&out, &relative).map_err(failed)?;
                let output = batch::publish(&target, &bytes).map_err(failed)?;
                Ok((output, bytes.len(), log))
            }))
            .unwrap_or_else(|_| Err(failed("batch conversion panicked; no output was published")));
            match result {
                Ok((output, bytes, steps)) => {
                    completed += 1;
                    let steps: Vec<Value> = steps.iter().map(step_json).collect();
                    files.push(json!({"path":path.to_string_lossy(),"status":"ok","output":output.to_string_lossy(),"bytes":bytes,"steps":steps}));
                }
                Err(e) => {
                    failed_count += 1;
                    files.push(json!({"path":path.to_string_lossy(),"status":"error","error":e.to_string()}));
                    stopped = stop;
                }
            }
        }
        let skipped = files.iter().filter(|file| file["status"] == "skipped").count();
        Ok(json!({"dry_run":dry_run,"completed":completed,"failed":failed_count,"skipped":skipped,"stopped":stopped,"files":files}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_engine::Session;

    #[test]
    fn doc_batch_dry_run_lists_outputs_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("input")).unwrap();
        let pdf = Session::new().create_from_text("Doc", "Hello").unwrap();
        std::fs::write(dir.path().join("input/a.pdf"), &*pdf).unwrap();
        let auto = Automation::new().with_root(dir.path()).unwrap();
        let report = auto.doc_batch(&Args(&json!({"paths":["input"],"steps":["optimize"],"out_dir":"out","dry_run":true}))).unwrap();
        assert_eq!(report["dry_run"], true, "{report}");
        assert_eq!(report["completed"], 0, "{report}");
        assert_eq!(report["failed"], 0, "{report}");
        assert_eq!(report["files"][0]["status"], "planned", "{report}");
        assert!(report["files"][0]["output"].as_str().unwrap().ends_with("out/a.pdf"), "{report}");
        assert!(!dir.path().join("out").exists());
    }
}
