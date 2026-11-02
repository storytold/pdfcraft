//! One-shot folder jobs shared by CLI and MCP. Discovery finishes before any output is made.

use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdfcraft_engine::{Session, ocr, optimize};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, failed};

const MAX_ENTRIES: usize = 10_000;
const MAX_DEPTH: usize = 64;
const MAX_INPUT: u64 = 512 * 1024 * 1024;

struct Recipe<'a> {
    steps: Vec<&'a str>,
    images: optimize::ImageSettings,
    ocr: ocr::OcrSettings,
}

impl<'a> Recipe<'a> {
    fn parse(auto: &Automation, args: &'a Args) -> Result<Self> {
        let steps = args.strs("steps")?;
        if steps.is_empty() || steps.len() > 3 {
            return Err(Args::wrong("steps", "one to three unique steps: convert, ocr, optimize"));
        }
        let mut seen = HashSet::new();
        for (i, step) in steps.iter().enumerate() {
            if !matches!(*step, "convert" | "ocr" | "optimize") || !seen.insert(*step) || (*step == "convert" && i != 0) {
                return Err(Args::wrong("steps", "unique convert/ocr/optimize steps, with convert first when present"));
            }
        }
        for (keys, step) in [(&["ppi", "quality"][..], "optimize"), (&["dpi", "language", "skip_text_pages"][..], "ocr")] {
            if !steps.contains(&step) && keys.iter().any(|k| args.get(k).is_some()) {
                return Err(failed(format!("{} settings need the {step} step", keys.join("/"))));
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
        Ok(Self { steps, images, ocr: auto.ocr_settings(args)? })
    }

    fn accepts(&self, path: &Path) -> bool {
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        extension == "pdf" || (self.steps.contains(&"convert") && pdfcraft_engine::CONVERTIBLE.contains(&extension.as_str()))
    }

    fn process(&self, path: &Path) -> Result<(Arc<Vec<u8>>, Vec<Value>)> {
        // A fresh session per file bounds retained document state and never touches caller tabs.
        let mut session = Session::new();
        let mut input = Vec::new();
        std::fs::File::open(path).map_err(failed)?.take(MAX_INPUT + 1).read_to_end(&mut input).map_err(failed)?;
        if input.len() as u64 > MAX_INPUT {
            return Err(failed("input exceeds 512 MiB"));
        }
        let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| failed("input name is not UTF-8"))?;
        let mut bytes = Arc::new(input);
        let mut log = Vec::new();
        for step in &self.steps {
            let before = bytes.len();
            match *step {
                "convert" => bytes = session.convert_to_pdf(name, &bytes)?.1,
                "ocr" => {
                    let engine = ocr::engine().map_err(failed)?;
                    let result = ocr::recognize_file(name, bytes, None, self.ocr.clone(), &engine, |_, _| true).map_err(failed)?;
                    log.push(
                        json!({"step":"ocr","words":result.words(),"skipped_pages":result.pages.iter().filter(|p| p.skipped.is_some()).count()}),
                    );
                    bytes = result.bytes;
                    continue;
                }
                "optimize" => {
                    let id = session.open(name, None, bytes.clone(), None).map_err(failed)?;
                    let settings = optimize::Settings { color: self.images, gray: self.images, ..Default::default() };
                    let (candidate, _) = session.optimized_bytes(id, &settings, &[])?;
                    session.close(id);
                    if candidate.len() < bytes.len() {
                        bytes = candidate;
                    }
                }
                _ => return Err(failed("unknown batch step")),
            }
            log.push(json!({"step":step,"bytes_before":before,"bytes_after":bytes.len()}));
        }
        Ok((bytes, log))
    }
}

// Windows junctions are reparse points too; do not descend into them as ordinary folders.
fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Canonicalize the existing ancestor, including when the requested output does not exist yet.
fn absolute(path: &Path) -> Result<PathBuf> {
    let mut ancestor = path;
    let mut tail = Vec::new();
    while !ancestor.exists() {
        if ancestor.symlink_metadata().is_ok() {
            return Err(failed("broken link in batch path"));
        }
        tail.push(ancestor.file_name().ok_or_else(|| failed("invalid batch path"))?);
        ancestor = ancestor.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    }
    let mut path = ancestor.canonicalize().map_err(failed)?;
    path.extend(tail.iter().rev());
    crate::lexical(&path).ok_or_else(|| failed("invalid batch path"))
}

struct Discovery {
    files: BTreeMap<PathBuf, PathBuf>,
    skipped: Vec<Value>,
    seen: HashSet<PathBuf>,
    visited: usize,
}

impl Discovery {
    fn walk(&mut self, path: &Path, relative: &Path, out: &Path, recursive: bool, depth: usize) -> Result<()> {
        self.visited += 1;
        if self.visited > MAX_ENTRIES || depth > MAX_DEPTH {
            return Err(failed("batch discovery exceeds 10000 entries or 64 folder levels"));
        }
        if depth > 0 && path.starts_with(out) {
            return Ok(());
        }
        let metadata = path.symlink_metadata().map_err(|e| failed(format!("{}: {e}", path.display())))?;
        if linked(&metadata) || (!metadata.is_file() && !metadata.is_dir()) {
            self.skipped.push(json!({"path":path.to_string_lossy(),"status":"skipped","reason":"link or special file"}));
        } else if (metadata.is_file() || depth == 0 || recursive) && self.seen.insert(path.to_path_buf()) {
            if metadata.is_file() {
                self.files.insert(path.to_path_buf(), relative.to_path_buf());
            } else if depth == 0 || recursive {
                // Bound the list before sorting it; never collect an unbounded folder listing.
                let mut entries = Vec::new();
                for entry in std::fs::read_dir(path).map_err(failed)? {
                    if entries.len() >= MAX_ENTRIES.saturating_sub(self.visited) {
                        return Err(failed("batch discovery exceeds 10000 entries"));
                    }
                    entries.push(entry.map_err(failed)?.path());
                }
                entries.sort();
                for child in entries {
                    let name = child.file_name().ok_or_else(|| failed("entry has no name"))?;
                    self.walk(&child, &relative.join(name), out, recursive, depth + 1)?;
                }
            }
        }
        Ok(())
    }
}

impl Automation {
    pub(crate) fn doc_batch(&self, args: &Args) -> Result<Value> {
        let recipe = Recipe::parse(self, args)?;
        let recursive = args.opt_bool("recursive")?.unwrap_or(false);
        let stop = args.opt_bool("stop_on_error")?.unwrap_or(false);
        let paths = args.strs("paths")?;
        if paths.is_empty() || paths.len() > 1000 {
            return Err(Args::wrong("paths", "1 to 1000 files or folders"));
        }
        let out = absolute(&self.resolve(args.str("out_dir")?, true)?)?;
        let mut roots = Vec::new();
        for path in paths {
            let resolved = self.resolve(path, false)?;
            let original = self.root.as_ref().map_or_else(|| PathBuf::from(path), |root| root.join(path));
            if linked(&original.symlink_metadata().map_err(failed)?) {
                return Err(failed("input roots must be regular files or folders, not links"));
            }
            let real = absolute(&resolved)?;
            if real.is_dir() && real.starts_with(&out) {
                return Err(failed("out_dir must not contain an input folder or equal it"));
            }
            roots.push(real);
        }
        let mut discovery = Discovery { files: BTreeMap::new(), skipped: Vec::new(), seen: HashSet::new(), visited: 0 };
        for root in roots {
            let relative = if root.is_dir() { PathBuf::new() } else { PathBuf::from(root.file_name().ok_or_else(|| failed("input has no name"))?) };
            discovery.walk(&root, &relative, &out, recursive, 0)?;
        }
        let mut files = discovery.skipped;
        let (mut completed, mut failed_count) = (0, 0);
        let mut stopped = false;
        for (path, relative) in discovery.files {
            if stopped || !recipe.accepts(&path) {
                files.push(json!({"path":path.to_string_lossy(),"status":"skipped","reason":if stopped {"stopped after error"} else {"unsupported input extension"}}));
                continue;
            }
            // Recheck confinement and links before reading a discovered file.
            let result: Result<_> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let current = self.resolve(path.to_str().ok_or_else(|| failed("input path is not UTF-8"))?, false)?;
                if linked(&path.symlink_metadata().map_err(failed)?) || absolute(&current)? != path {
                    return Err(failed("input path changed after discovery"));
                }
                let (bytes, log) = recipe.process(&current)?;
                let target = out.join(relative).with_extension("pdf");
                let target = self.resolve(target.to_str().ok_or_else(|| failed("output path is not UTF-8"))?, true)?;
                if !target.starts_with(&out) {
                    return Err(failed("output path leaves out_dir"));
                }
                let output = publish(&target, &bytes)?;
                Ok((output, bytes.len(), log))
            }))
            .unwrap_or_else(|_| Err(failed("batch conversion panicked; no output was published")));
            match result {
                Ok((output, bytes, steps)) => {
                    completed += 1;
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

/// Publish atomically without replacing any existing name, even if another writer wins a race.
fn publish(target: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let dir = target.parent().ok_or_else(|| failed("output has no parent"))?;
    std::fs::create_dir_all(dir).map_err(failed)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(failed)?;
    temp.write_all(bytes).map_err(failed)?;
    temp.as_file().sync_all().map_err(failed)?;
    let stem = target.file_stem().ok_or_else(|| failed("output has no stem"))?.to_string_lossy();
    for n in 1..=10_000 {
        let candidate = if n == 1 { target.to_path_buf() } else { dir.join(format!("{stem} ({n}).pdf")) };
        match temp.persist_noclobber(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists || candidate.symlink_metadata().is_ok() => temp = e.file,
            Err(e) => return Err(failed(e.error)),
        }
    }
    Err(failed("all 10000 output names are occupied"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_limits_fail_before_reading_another_entry() {
        let mut discovery = Discovery { files: BTreeMap::new(), skipped: Vec::new(), seen: HashSet::new(), visited: MAX_ENTRIES };
        assert!(discovery.walk(Path::new("absent"), Path::new(""), Path::new("output"), true, 0).unwrap_err().to_string().contains("10000"));
        discovery.visited = 0;
        assert!(discovery.walk(Path::new("absent"), Path::new(""), Path::new("output"), true, MAX_DEPTH + 1).unwrap_err().to_string().contains("64"));
    }
}
