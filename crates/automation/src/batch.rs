//! One-shot folder jobs shared by CLI and MCP: the arguments and `--root` confinement. Finding
//! the files, running the steps and publishing without replacing anything is
//! `pdfcraft_engine::batch`, which the desktop app uses too.

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
    if let Some(ppi) = args.opt_num("ppi")? {
        if !(9.0..=1600.0).contains(&ppi) {
            return Err(Args::wrong("ppi", "9 to 1600"));
        }
        images.target_ppi = ppi;
        images.above_ppi = ppi * 1.5;
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
    Ok(Recipe { steps, images, ocr: auto.ocr_settings(args)? })
}

fn step_json(log: &StepLog) -> Value {
    match log {
        StepLog::Ocr { words, skipped_pages } => json!({"step":"ocr","words":words,"skipped_pages":skipped_pages}),
        StepLog::Size { step, before, after } => json!({"step":step.name(),"bytes_before":before,"bytes_after":after}),
    }
}

impl Automation {
    pub(crate) fn doc_batch(&self, args: &Args) -> Result<Value> {
        let recipe = recipe(self, args)?;
        let recursive = args.opt_bool("recursive")?.unwrap_or(false);
        let stop = args.opt_bool("stop_on_error")?.unwrap_or(false);
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
        for (path, relative) in discovery.files {
            if stopped || !recipe.accepts(&path) {
                let why = if stopped { SkipReason::Stopped } else { SkipReason::Unsupported };
                files.push(json!({"path":path.to_string_lossy(),"status":"skipped","reason":why.text()}));
                continue;
            }
            // Recheck confinement and links before reading a discovered file.
            let result: Result<_> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let current = self.resolve(path.to_str().ok_or_else(|| failed("input path is not UTF-8"))?, false)?;
                if batch::linked(&path.symlink_metadata().map_err(failed)?) || batch::absolute(&current).map_err(failed)? != path {
                    return Err(failed("input path changed after discovery"));
                }
                let (bytes, log) = recipe.process(&current, |_, _| true).map_err(failed)?;
                let target = batch::output_path(&out, &relative);
                let target = self.resolve(target.to_str().ok_or_else(|| failed("output path is not UTF-8"))?, true)?;
                if !target.starts_with(&out) {
                    return Err(failed("output path leaves out_dir"));
                }
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
        let skipped = files.len().saturating_sub(completed + failed_count);
        Ok(json!({"completed":completed,"failed":failed_count,"skipped":skipped,"stopped":stopped,"files":files}))
    }
}
