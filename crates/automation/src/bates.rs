use std::io::Write;

use pdfcraft_engine::{Edit, HeaderFooter};
use serde_json::{Value, json};

use super::error::{Result, ToolError, failed};
use super::json::Args;
use super::paths::read_input;
use super::{Arc, Automation, Path, PathBuf};

const MAX_BATES_BYTES: u64 = 512 * 1024 * 1024;

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
            Some(value) => {
                value.as_u64().filter(|n| *n >= 1).ok_or_else(|| ToolError::InvalidArgs("start_number must be an integer of at least 1".into()))?
            }
        };
        let digits = args.opt_int("digits")?.unwrap_or(6);
        let digits =
            u8::try_from(digits).ok().filter(|d| (1..=12).contains(d)).ok_or_else(|| ToolError::InvalidArgs("digits must be from 1 to 12".into()))?;
        let prefix = args.opt_str("prefix")?.unwrap_or("");
        let suffix = args.opt_str("suffix")?.unwrap_or("");
        if prefix.chars().count() > 128
            || suffix.chars().count() > 128
            || prefix.contains('#')
            || suffix.contains('#')
            || prefix.contains(">>")
            || suffix.contains(">>")
        {
            return Err(ToolError::InvalidArgs("prefix and suffix must be at most 128 characters and cannot contain # or >>".into()));
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
        check_unique_outputs(&targets, dry_run)?;

        let mut plans = Vec::with_capacity(files.len());
        let mut next = start;
        let mut input_bytes = 0u64;
        for (index, ((input_name, input_path), (_, output_path))) in sources.iter().zip(&targets).enumerate() {
            let size = std::fs::metadata(input_path).map_err(failed)?.len();
            input_bytes = input_bytes
                .checked_add(size)
                .filter(|total| *total <= MAX_BATES_BYTES)
                .ok_or_else(|| failed("combined Bates input exceeds the 512 MiB safety limit"))?;
            let bytes = read_input(input_path)?;
            input_bytes = input_bytes
                .saturating_sub(size)
                .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
                .filter(|total| *total <= MAX_BATES_BYTES)
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
            let last_offset = count.checked_sub(1).ok_or_else(|| failed(format!("{input_name} has no pages to number")))?;
            let last = next.checked_add(last_offset).ok_or_else(|| failed(format!("Bates number range overflows for {input_name}")))?;
            plans.push(FilePlan {
                input: input_name.clone(),
                source: input_path.clone(),
                password: passwords.get(index).cloned().flatten(),
                output: output_path.clone(),
                pages,
                first: next,
                last,
            });
            if index + 1 < files.len() {
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
                    let candidate = output_bytes
                        .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
                        .filter(|total| *total <= MAX_BATES_BYTES);
                    if let Some(total) = candidate {
                        output_bytes = total;
                        staged.push((plan.output.clone(), bytes));
                    } else {
                        results.push(json!({"input":plan.input,"output":plan.output,"status":"error","pages":plan.pages,"first":plan.first,"last":plan.last,"error":"combined Bates output exceeds the 512 MiB safety limit"}));
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

fn check_unique_outputs(targets: &[(String, PathBuf)], dry_run: bool) -> Result<()> {
    if targets.len() < 2 {
        return Ok(());
    }

    // Probe exact basenames in a directory under each canonical parent, so the filesystem applies its own name rules.
    let mut groups: std::collections::HashMap<PathBuf, (PathBuf, Vec<(String, std::ffi::OsString)>)> = std::collections::HashMap::new();
    for (name, target) in targets {
        let parent = target.parent().ok_or_else(|| failed(format!("output has no parent directory: {name}")))?;
        let identity = parent.canonicalize().map_err(failed)?;
        let probe_name = output_probe_name(target).map_err(failed)?;
        groups.entry(identity.clone()).or_insert_with(|| (identity, Vec::new())).1.push((name.clone(), probe_name));
    }

    if dry_run {
        // Dry runs cannot probe the volume: Windows uses a conservative case-insensitive comparison,
        // while Unix preserves case, so unusual case-sensitive Windows or case-insensitive Unix volumes can differ.
        for (_, (_, names)) in groups {
            for (index, (name, candidate)) in names.iter().enumerate() {
                if names.iter().take(index).any(|(_, prior)| same_output_name(prior, candidate)) {
                    return Err(failed(format!("two files use the same output path: {name}")));
                }
            }
        }
        return Ok(());
    }

    for (_, (parent, names)) in groups {
        match tempfile::tempdir_in(&parent) {
            Ok(probes) => {
                for (name, probe_name) in names {
                    let probe = probes.path().join(probe_name);
                    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
                        Ok(file) => drop(file),
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                            return Err(failed(format!("two files use the same output path: {name}")));
                        }
                        Err(error) => return Err(failed(format!("could not validate output path {name}: {error}"))),
                    }
                }
            }
            #[cfg(windows)]
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => probe_files_for_resolved_outputs(&parent, names)?,
            Err(error) => return Err(failed(error)),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn probe_files_for_resolved_outputs(parent: &Path, names: Vec<(String, std::ffi::OsString)>) -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
    let mut probes = Vec::with_capacity(names.len());
    for (name, probe_name) in names {
        let probe = parent.join(probe_name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true).share_mode(0).custom_flags(FILE_FLAG_DELETE_ON_CLOSE);
        match options.open(&probe) {
            Ok(file) => probes.push(file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(failed(format!("two files use the same output path: {name}")));
            }
            Err(error) => return Err(failed(format!("could not validate output path {name}: {error}"))),
        }
    }
    drop(probes);
    Ok(())
}

fn same_output_name(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        fn key(name: &std::ffi::OsStr) -> Vec<u32> {
            std::char::decode_utf16(name.encode_wide())
                .map(|decoded| match decoded {
                    Ok(character) => {
                        let mut uppercase = character.to_uppercase();
                        match (uppercase.next(), uppercase.next()) {
                            (Some(single), None) => single as u32,
                            _ => character as u32,
                        }
                    }
                    Err(error) => 0x11_0000 + u32::from(error.unpaired_surrogate()),
                })
                .collect()
        }
        return key(left) == key(right);
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn output_probe_name(path: &Path) -> std::io::Result<std::ffi::OsString> {
    let name = path.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no file name"))?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let is_verbatim = path.components().any(|component| matches!(component, std::path::Component::Prefix(prefix) if matches!(prefix.kind(), std::path::Prefix::Verbatim(_) | std::path::Prefix::VerbatimUNC(_, _) | std::path::Prefix::VerbatimDisk(_))));
        let mut wide: Vec<u16> = name.encode_wide().collect();
        if !is_verbatim {
            while wide.last().is_some_and(|unit| *unit == u16::from(b'.') || *unit == u16::from(b' ')) {
                wide.pop();
            }
        }
        return Ok(std::ffi::OsString::from_wide(&wide));
    }
    #[cfg(not(windows))]
    {
        Ok(name.to_os_string())
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

#[cfg(test)]
mod tests {
    use super::check_unique_outputs;

    #[test]
    fn dry_run_path_check_creates_no_probe_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("same.pdf");
        let targets = vec![("first.pdf".into(), path.clone()), ("second.pdf".into(), path)];
        assert!(check_unique_outputs(&targets, true).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn dry_run_detects_output_parent_aliases_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(dir.path(), &alias).unwrap();
        let first = dir.path().join("same.pdf");
        let second = alias.join("same.pdf");
        let targets = vec![("first.pdf".into(), first), ("second.pdf".into(), second)];
        assert!(check_unique_outputs(&targets, true).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn long_output_names_fit_in_probe_directory() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join(format!("{}.pdf", "a".repeat(220)));
        let second = dir.path().join(format!("{}.pdf", "b".repeat(220)));
        let targets = vec![("first.pdf".into(), first), ("second.pdf".into(), second)];
        assert!(check_unique_outputs(&targets, false).is_ok());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn output_probe_paths_preserve_non_utf8_names() {
        use std::os::unix::ffi::OsStringExt;
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join(std::ffi::OsString::from_vec(vec![b'o', 0xff]));
        let second = dir.path().join(std::ffi::OsString::from_vec(vec![b'o', 0xfe]));
        let targets = vec![("first.pdf".into(), first), ("second.pdf".into(), second)];
        assert!(check_unique_outputs(&targets, false).is_ok());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn windows_probe_names_trim_only_non_verbatim_paths() {
        use std::ffi::OsString;
        use std::path::Path;

        let normal = Path::new(r"C:\root\report.pdf. ");
        assert_eq!(super::output_probe_name(normal).unwrap(), OsString::from("report.pdf"));

        let verbatim = Path::new(r"\\?\C:\root\report.pdf. ");
        assert_eq!(super::output_probe_name(verbatim).unwrap(), OsString::from("report.pdf. "));
    }

    #[cfg(windows)]
    #[test]
    fn dry_run_detects_case_only_output_collisions_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("Case.pdf");
        let second = dir.path().join("CASE.pdf");
        let targets = vec![("first.pdf".into(), first), ("second.pdf".into(), second)];
        assert!(check_unique_outputs(&targets, true).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn permission_denied_child_directory_uses_delete_on_close_file_probes() {
        use std::process::Command;

        let dir = tempfile::tempdir().unwrap();
        let identity = Command::new("whoami.exe").args(["/user", "/fo", "csv", "/nh"]).output().unwrap();
        assert!(identity.status.success());
        let identity = String::from_utf8(identity.stdout).unwrap();
        let sid = identity.split('"').nth(3).unwrap();
        let acl = Command::new("icacls.exe").arg(dir.path()).arg("/deny").arg(format!("*{sid}:(AD)")).status().unwrap();
        assert!(acl.success());

        assert!(matches!(tempfile::tempdir_in(dir.path()), Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied));
        let direct_probe = dir.path().join("direct-probe");
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&direct_probe).unwrap();
        drop(file);
        std::fs::remove_file(&direct_probe).unwrap();

        let targets = vec![("first.pdf".into(), dir.path().join("first.pdf")), ("second.pdf".into(), dir.path().join("second.pdf"))];
        assert!(check_unique_outputs(&targets, false).is_ok());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
