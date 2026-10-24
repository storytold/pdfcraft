//! PDFCraft's open project manifest. This is not an ABBYY proprietary OCR project.

use super::*;

pub(super) fn local_paths(source: &ProjectSource, root: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    let (base, manifest) = manifest(source, root)?;
    manifest.files.iter().map(|relative| confined(&base.join(relative), root)).collect()
}

/// An unprocessed missing project entry is an error, never a silently truncated
/// grouped output. Completed move/delete entries stay in the immutable manifest.
pub(super) fn validate_and_order(
    source: &ProjectSource,
    input: &Path,
    root: Option<&Path>,
    journal: &Journal,
    report: &mut StageReport,
) -> Result<(), String> {
    let (base, manifest) = manifest(source, root)?;
    for relative in &manifest.files {
        if fs::symlink_metadata(base.join(relative)).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            && !journal.records.values().any(|record| record.remote.relative == *relative)
            && !archived_relative(input, &journal.config_hash, relative)?
                .is_some_and(|record| record.acknowledged && record.remote.relative == *relative)
        {
            return Err("project contains a missing, unprocessed source; restore it before processing the project".into());
        }
    }
    let positions = manifest.files.iter().enumerate().map(|(index, path)| (path, index)).collect::<BTreeMap<_, _>>();
    if report.items.iter().any(|item| !positions.contains_key(&item.relative)) {
        return Err("project manifest changed while staged inputs are pending; restore its entries or use a fresh task".into());
    }
    report.items.sort_by_key(|item| positions.get(&item.relative).copied().unwrap_or(usize::MAX));
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    /// Must be "pdfcraft-hot-folder-project".
    pub format: String,
    pub version: u32,
    /// Ordered PDF/PNG/JPEG/TIFF paths relative to this manifest, without parent traversal.
    pub files: Vec<PathBuf>,
}

impl Default for ProjectManifest {
    fn default() -> Self {
        Self { format: "pdfcraft-hot-folder-project".into(), version: 1, files: Vec::new() }
    }
}

pub fn write_project_manifest(path: &Path, manifest: &ProjectManifest, root: Option<&Path>) -> Result<(), String> {
    manifest_check(manifest)?;
    let parent = confined(path.parent().ok_or("project manifest needs a parent directory")?, root)?;
    let filename = path.file_name().ok_or("project manifest needs a filename")?;
    for file in &manifest.files {
        confined(&parent.join(file), root)?;
    }
    let bytes = serde_json::to_vec_pretty(manifest).map_err(io_error)?;
    if bytes.len() as u64 > MAX_JSON {
        return Err("project manifest exceeds 8 MiB".into());
    }
    let mut temporary = tempfile::Builder::new().prefix(".pdfcraft-project-").tempfile_in(&parent).map_err(io_error)?;
    temporary.write_all(&bytes).and_then(|_| temporary.as_file().sync_all()).map_err(io_error)?;
    temporary.persist_noclobber(parent.join(filename)).map_err(|_| "project manifest already exists or could not be saved; no file was replaced")?;
    sync_directory(&parent)
}

fn manifest_check(manifest: &ProjectManifest) -> Result<(), String> {
    if manifest.format != "pdfcraft-hot-folder-project" || manifest.version != 1 {
        return Err("use PDFCraft's open version-1 project manifest; proprietary ABBYY OCR projects are not supported".into());
    }
    if manifest.files.is_empty() || manifest.files.len() > MAX_ITEMS {
        return Err("project must contain 1..10000 PDF, PNG, JPEG or TIFF files".into());
    }
    let mut unique = std::collections::BTreeSet::new();
    for path in &manifest.files {
        if !relative_ok(path) || !supported(path) || !unique.insert(path) {
            return Err("project paths must be unique relative PDF/image paths without traversal".into());
        }
    }
    Ok(())
}

fn manifest(source: &ProjectSource, root: Option<&Path>) -> Result<(PathBuf, ProjectManifest), String> {
    let path = confined(&source.manifest, root)?;
    let bytes = read_bounded(&path, MAX_JSON)?;
    let parsed: ProjectManifest =
        serde_json::from_slice(&bytes).map_err(|_| "invalid PDFCraft project manifest; ABBYY OCR projects are not supported".to_owned())?;
    manifest_check(&parsed)?;
    Ok((path.parent().ok_or("project manifest needs a parent directory")?.to_owned(), parsed))
}

pub(super) fn validate(source: &ProjectSource, input: &Path, root: Option<&Path>) -> Result<(), String> {
    let (base, parsed) = manifest(source, root)?;
    for relative in parsed.files {
        let path = confined(&base.join(relative), root)?;
        if path.starts_with(input) {
            return Err("project originals must be outside their staging inbox".into());
        }
    }
    if let SourceAction::Move { destination } = &source.action {
        let archive = confined(Path::new(destination), root)?;
        if !archive.is_dir() || archive.starts_with(input) || input.starts_with(&archive) {
            return Err("project archive must be an existing folder disjoint from staging".into());
        }
    }
    Ok(())
}

pub(super) struct Client<'a> {
    source: &'a ProjectSource,
    root: Option<&'a Path>,
    base: PathBuf,
    manifest: ProjectManifest,
}
impl<'a> Client<'a> {
    pub(super) fn new(source: &'a ProjectSource, root: Option<&'a Path>) -> Result<Self, String> {
        let (base, manifest) = manifest(source, root)?;
        Ok(Self { source, root, base, manifest })
    }
    fn path(&self, file: &RemoteFile) -> Result<PathBuf, String> {
        if !self.manifest.files.contains(&file.relative) {
            return Err("project receipt is not listed in the manifest".into());
        }
        let path = confined(&self.base.join(&file.relative), self.root)?;
        if path.to_string_lossy() != file.key {
            return Err("project receipt resolved to another source path".into());
        }
        Ok(path)
    }
}
impl Backend for Client<'_> {
    fn list(&mut self, control: &Control<'_>) -> Result<Vec<RemoteFile>, String> {
        let mut result = Vec::new();
        for relative in &self.manifest.files {
            control.check()?;
            let candidate = self.base.join(relative);
            // A previous acknowledged move/delete leaves the immutable manifest intact.
            if fs::symlink_metadata(&candidate).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) {
                continue;
            }
            let path = confined(&candidate, self.root)?;
            let revision = file_hash(&path, control)?;
            result.push(RemoteFile {
                key: path.to_string_lossy().into_owned(),
                relative: relative.clone(),
                revision,
                size: fs::metadata(&path).map_err(io_error)?.len(),
                group: None,
            });
        }
        Ok(result)
    }
    fn read(&mut self, file: &RemoteFile, writer: &mut dyn Write, control: &Control<'_>) -> Result<(), String> {
        let path = self.path(file)?;
        if file_hash(&path, control)? != file.revision {
            return Err("project source changed before staging".into());
        }
        let count = copy_limited(&mut File::open(&path).map_err(io_error)?, writer, file.size, control)?;
        if count != file.size || file_hash(&path, control)? != file.revision {
            return Err("project source changed during staging".into());
        }
        Ok(())
    }
    fn finish(&mut self, files: &[Record], action: &SourceAction, control: &Control<'_>) -> Result<(), String> {
        let _ = &self.source;
        for record in files {
            control.check()?;
            let candidate = self.base.join(&record.remote.relative);
            let exists = !fs::symlink_metadata(&candidate).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
            let path = if exists { Some(self.path(&record.remote)?) } else { None };
            if let Some(path) = &path
                && file_hash(path, control)? != record.source.sha256
            {
                return Err("project original changed after processing; original retained".into());
            }
            match action {
                SourceAction::Keep => {}
                SourceAction::Delete => {
                    if let Some(path) = path {
                        fs::remove_file(&path).map_err(io_error)?;
                        sync_directory(path.parent().ok_or("project original needs a parent directory")?)?;
                    }
                }
                SourceAction::Move { destination } => {
                    let archive = confined(Path::new(destination), self.root)?;
                    let target = archive.join(&record.source.staged_relative);
                    if target.exists() {
                        if file_hash(&target, control)? != record.source.sha256 {
                            return Err("project archive filename is occupied; original retained".into());
                        }
                    } else {
                        let source = path.as_ref().ok_or("project original and archive are both missing")?;
                        let mut temporary = tempfile::Builder::new().prefix(".pdfcraft-project-").tempfile_in(&archive).map_err(io_error)?;
                        copy_limited(&mut File::open(source).map_err(io_error)?, &mut temporary, MAX_FILE, control)?;
                        temporary.as_file().sync_all().map_err(io_error)?;
                        if file_hash(temporary.path(), control)? != record.source.sha256 {
                            return Err("project original changed during archive copy; original retained".into());
                        }
                        temporary.persist_noclobber(&target).map_err(|_| "project archive filename became occupied; original retained")?;
                    }
                    // Also confirm an existing matching archive on recovery; a previous
                    // process may have stopped between its rename and directory sync.
                    sync_directory(&archive)?;
                    if let Some(path) = path {
                        if file_hash(&path, control)? != record.source.sha256 {
                            return Err("project original changed before removal; original retained".into());
                        }
                        fs::remove_file(&path).map_err(io_error)?;
                        sync_directory(path.parent().ok_or("project original needs a parent directory")?)?;
                    }
                }
            }
        }
        Ok(())
    }
}
