use std::io::Write;

use pdfcraft_engine::{Edit, HeaderFooter};
use serde_json::{Value, json};

use super::error::{Result, ToolError, failed};
use super::json::Args;
use super::paths::read_input;
use super::{Arc, Automation, Path, PathBuf};

struct FilePlan {
    input: String,
    source: PathBuf,
    password: Option<String>,
    output: PathBuf,
    pages: usize,
    first: u64,
    last: u64,
}

impl Automation {
    pub(crate) fn doc_bates_multi(&mut self, args: &Args) -> Result<Value> {
        let files = args.0.get("files").and_then(Value::as_array).ok_or_else(|| ToolError::InvalidArgs("files must be an array".into()))?;
        if files.is_empty() || files.len() > 1000 {
            return Err(ToolError::InvalidArgs("files must contain 1 to 1000 entries".into()));
        }
        let start = match args.0.get("start_number") {
            None | Some(Value::Null) => 1,
            Some(value) => value
                .as_u64()
                .filter(|n| *n >= 1)
                .ok_or_else(|| ToolError::InvalidArgs("start_number must be a whole number of at least 1".into()))?,
        };
        let digits = args.opt_int("digits")?.unwrap_or(6);
        let digits =
            u8::try_from(digits).ok().filter(|d| (1..=12).contains(d)).ok_or_else(|| ToolError::InvalidArgs("digits must be from 1 to 12".into()))?;
        let prefix = args.opt_str("prefix")?.unwrap_or("");
        let suffix = args.opt_str("suffix")?.unwrap_or("");
        // `#` separates the token's fields and `>>` ends it: either would cut the token short.
        if prefix.len() > 128 || suffix.len() > 128 || [prefix, suffix].iter().any(|s| s.contains('#') || s.contains(">>")) {
            return Err(ToolError::InvalidArgs("prefix and suffix must be at most 128 bytes and cannot contain # or >>".into()));
        }
        let dry_run = args.opt_bool("dry_run")?.unwrap_or(false);

        // Resolve and reserve the complete path list before opening or writing anything.
        let mut sources = Vec::with_capacity(files.len());
        let mut targets = Vec::with_capacity(files.len());
        let mut passwords = Vec::with_capacity(files.len());
        for entry in files {
            let input = entry
                .get("input")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ToolError::InvalidArgs("each file needs a non-empty input path".into()))?;
            let output = entry
                .get("output")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ToolError::InvalidArgs("each file needs a non-empty output path".into()))?;
            let source = self.resolve(input, false)?;
            let target = self.resolve(output, true)?;
            if target.exists() {
                return Err(failed(format!("output already exists; refusing to replace it: {output}")));
            }
            let parent = target.parent().ok_or_else(|| failed(format!("output has no parent directory: {output}")))?;
            if !parent.is_dir() {
                return Err(failed(format!("output directory does not exist: {}", parent.display())));
            }
            sources.push((input.to_string(), source));
            targets.push((output.to_string(), target));
            passwords.push(entry.get("password").and_then(Value::as_str).map(str::to_owned));
        }
        let input_paths: std::collections::HashSet<_> = sources.iter().map(|(_, path)| path.clone()).collect();
        let mut output_paths = std::collections::HashSet::new();
        for (name, path) in &targets {
            if input_paths.contains(path) {
                return Err(failed(format!("output {name} names an input file; choose a separate output path")));
            }
            if !output_paths.insert(path.clone()) {
                return Err(failed(format!("two files use the same output path: {name}")));
            }
        }

        let mut plans = Vec::with_capacity(files.len());
        let mut next = start;
        let mut input_bytes = 0u64;
        for (index, ((input_name, input_path), (_, output_path))) in sources.iter().zip(&targets).enumerate() {
            let size = std::fs::metadata(input_path).map_err(failed)?.len();
            input_bytes = input_bytes
                .checked_add(size)
                .filter(|total| *total <= 512 * 1024 * 1024)
                .ok_or_else(|| failed("combined Bates input exceeds the 512 MiB safety limit"))?;
            let bytes = read_input(input_path)?;
            input_bytes = input_bytes
                .saturating_sub(size)
                .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
                .filter(|total| *total <= 512 * 1024 * 1024)
                .ok_or_else(|| failed("combined Bates input exceeds the 512 MiB safety limit"))?;
            let display_name = input_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let password = passwords.get(index).and_then(Option::as_deref);
            let id = self.session.open(display_name, Some(input_path.to_string_lossy().into_owned()), Arc::new(bytes), password).map_err(failed)?;
            let pages =
                self.session.get(id).map(|doc| doc.info.pages.len()).ok_or_else(|| failed("opened document disappeared during Bates preflight"))?;
            self.session.close(id);
            if pages == 0 {
                return Err(failed(format!("{input_name} has no pages to number")));
            }
            let count = u64::try_from(pages).map_err(|_| failed(format!("page count is too large for {input_name}")))?;
            let last = next.checked_add(count.saturating_sub(1)).ok_or_else(|| failed(format!("Bates number range overflows for {input_name}")))?;
            plans.push(FilePlan {
                input: input_name.clone(),
                source: input_path.clone(),
                password: passwords.get(index).cloned().flatten(),
                output: output_path.clone(),
                pages,
                first: next,
                last,
            });
            if index + 1 < sources.len() {
                next = last.checked_add(1).ok_or_else(|| failed(format!("Bates sequence overflows after {input_name}")))?;
            }
        }

        let intervals: Vec<Value> = plans
            .iter()
            .zip(&targets)
            .map(|(p, (name, _))| json!({"input":p.input,"output":name,"pages":p.pages,"first":p.first,"last":p.last}))
            .collect();
        if dry_run {
            return Ok(json!({"dry_run":true,"files":intervals}));
        }

        let mut staged = Vec::with_capacity(plans.len());
        let mut results = Vec::with_capacity(plans.len());
        let mut output_bytes = 0u64;
        for plan in &plans {
            let source_bytes = match read_input(&plan.source) {
                Ok(bytes) => bytes,
                Err(error) => {
                    results.push(json!({"input":plan.input,"output":plan.output,"status":"error","pages":plan.pages,"first":plan.first,"last":plan.last,"error":error.to_string()}));
                    continue;
                }
            };
            let name = plan.source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let id = match self.session.open(name, Some(plan.source.to_string_lossy().into_owned()), Arc::new(source_bytes), plan.password.as_deref())
            {
                Ok(id) => id,
                Err(error) => {
                    results.push(json!({"input":plan.input,"output":plan.output,"status":"error","pages":plan.pages,"first":plan.first,"last":plan.last,"error":error.to_string()}));
                    continue;
                }
            };
            if self.session.get(id).is_none_or(|doc| doc.info.pages.len() != plan.pages) {
                self.session.close(id);
                results.push(json!({"input":plan.input,"output":plan.output,"status":"error","pages":plan.pages,"first":plan.first,"last":plan.last,"error":"input changed after preflight; page count differs"}));
                continue;
            }
            let template = format!("<<Bates Number#{digits}#{}#{prefix}#{suffix}>>", plan.first);
            let mut settings = HeaderFooter::default();
            settings.text[4] = template;
            let pages: Vec<usize> = (0..plan.pages).collect();
            let applied = self.session.apply(id, Edit::AddHeaderFooter { pages, settings, replace: false });
            let result = applied.and_then(|()| self.session.save_full_bytes(id));
            match result {
                Ok(bytes) => {
                    let total = output_bytes.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                    if total > 512 * 1024 * 1024 {
                        results.push(json!({"input":plan.input,"output":plan.output,"status":"error","pages":plan.pages,"first":plan.first,"last":plan.last,"error":"combined Bates output exceeds the 512 MiB safety limit"}));
                    } else {
                        output_bytes = total;
                        staged.push((plan.output.clone(), bytes));
                    }
                }
                Err(error) => results.push(json!({"input":plan.input,"output":plan.output,"status":"error","pages":plan.pages,"first":plan.first,"last":plan.last,"error":error.to_string()})),
            }
            self.session.close(id);
        }
        for (path, bytes) in staged {
            match publish_new(&path, &bytes) {
                Ok(()) => {
                    let plan = plans.iter().find(|p| p.output == path);
                    if let Some(plan) = plan {
                        results.push(json!({"input":plan.input,"output":path,"status":"ok","first":plan.first,"last":plan.last,"pages":plan.pages}));
                    }
                }
                Err(error) => {
                    let plan = plans.iter().find(|p| p.output == path);
                    if let Some(plan) = plan {
                        results.push(json!({"input":plan.input,"output":path,"status":"error","pages":plan.pages,"first":plan.first,"last":plan.last,"error":error.to_string()}));
                    }
                }
            }
        }
        results.sort_by_key(|result| plans.iter().position(|plan| result["output"] == json!(plan.output)).unwrap_or(usize::MAX));
        let completed = results.iter().filter(|r| r["status"] == "ok").count();
        let failed_count = results.len().saturating_sub(completed);
        Ok(json!({"dry_run":false,"completed":completed,"failed":failed_count,"files":results}))
    }
}

/// Publish a PDF atomically while refusing a target created by another process after preflight.
fn publish_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| failed("output has no parent directory"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(failed)?;
    temp.write_all(bytes).map_err(failed)?;
    temp.as_file().sync_all().map_err(failed)?;
    temp.persist_noclobber(path).map(|_| ()).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            failed(format!("output appeared after preflight; refusing to replace it: {}", path.display()))
        } else {
            failed(error.error)
        }
    })
}
