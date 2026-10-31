//! Hot Folder output planning and conversion. Sources are never modified here.
//!
//! Every conversion is staged before publishing any file. Each publication uses
//! atomic no-clobber persistence. Since different destinations can be on different
//! volumes, a publication failure reports the complete files already saved; only a
//! successful receipt authorizes the caller's configured source move/delete step.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::export::{Exporter, ImageFormat};
use crate::{Session, hot_folder::confined};

#[path = "hot_folder_optimization.rs"]
pub mod optimization;

const MAX_INPUT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_STAGED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_OUTPUT_FILES: usize = 10_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grouping {
    #[default]
    File,
    Folder,
    All,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// Keep the processed PDF, including its OCR text if recognition was enabled.
    #[default]
    Pdf,
    /// Render all visible content; create an image-only PDF without hidden OCR text.
    ImagePdf,
    Docx,
    Html,
    Rtf,
    Xlsx,
    Epub,
    Analysis,
    Txt,
    Png,
    Jpeg,
    Tiff,
}

impl Format {
    pub const ALL: &[Self] = &[
        Self::Pdf,
        Self::ImagePdf,
        Self::Docx,
        Self::Html,
        Self::Rtf,
        Self::Xlsx,
        Self::Epub,
        Self::Txt,
        Self::Png,
        Self::Jpeg,
        Self::Tiff,
        Self::Analysis,
    ];
    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf | Self::ImagePdf => "pdf",
            Self::Docx => "docx",
            Self::Html => "html",
            Self::Rtf => "rtf",
            Self::Xlsx => "xlsx",
            Self::Epub => "epub",
            Self::Analysis => "json",
            Self::Txt => "txt",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Pdf => "PDF (processed document)",
            Self::ImagePdf => "PDF (image only)",
            Self::Docx => "Word (DOCX)",
            Self::Html => "HTML",
            Self::Rtf => "Rich text (RTF)",
            Self::Xlsx => "Excel (XLSX)",
            Self::Epub => "E-book (EPUB)",
            Self::Analysis => "Layout report (JSON)",
            Self::Txt => "Plain text (TXT)",
            Self::Png => "PNG images",
            Self::Jpeg => "JPEG images",
            Self::Tiff => "TIFF images",
        }
    }
    fn image(self, quality: u8) -> Option<ImageFormat> {
        match self {
            Self::Png => Some(ImageFormat::Png),
            Self::Jpeg => Some(ImageFormat::Jpeg { quality }),
            Self::Tiff => Some(ImageFormat::Tiff),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SaveStep {
    /// None uses the task's output directory. Explicit folders must already exist.
    pub folder: Option<PathBuf>,
    pub format: Format,
    pub grouping: Grouping,
    /// File stem, without extension. Tokens: {name}, {folder}, {task}, {date}, {time}, {counter}.
    /// Date/time use the batch's UTC timestamp, so all steps agree. Counter is one-based.
    pub name_template: String,
    pub dpi: f64,
    pub jpeg_quality: u8,
    /// EPUB language tag; "und" means undetermined. The scheduler fills its OCR language.
    pub language: String,
    /// Optional optimization of final PDF bytes, after grouping and image-only conversion.
    pub optimization: optimization::Settings,
}

impl Default for SaveStep {
    fn default() -> Self {
        Self {
            folder: None,
            format: Format::Pdf,
            grouping: Grouping::File,
            name_template: "{name}".into(),
            dpi: 150.0,
            jpeg_quality: 90,
            language: "und".into(),
            optimization: optimization::Settings::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputPlan {
    pub steps: Vec<SaveStep>,
}

impl Default for OutputPlan {
    fn default() -> Self {
        Self { steps: vec![SaveStep::default()] }
    }
}

impl OutputPlan {
    pub fn needs_batch(&self) -> bool {
        self.steps.iter().any(|step| step.grouping != Grouping::File)
    }

    /// Validate before reading source files. The caller also checks destinations against
    /// its full set of input folders to prevent recursive output/input loops.
    pub fn validate(&self, default_output: &Path, root: Option<&Path>) -> Result<(), String> {
        self.validate_settings()?;
        for step in &self.steps {
            let folder = confined(step.folder.as_deref().unwrap_or(default_output), root)?;
            if !folder.is_dir() {
                return Err(format!("output folder does not exist: {}", folder.display()));
            }
        }
        Ok(())
    }

    pub fn validate_settings(&self) -> Result<(), String> {
        if self.steps.is_empty() || self.steps.len() > 16 {
            return Err("choose between 1 and 16 output saving steps".into());
        }
        for step in &self.steps {
            step.optimization.validate()?;
            if step.optimization.enabled() && !matches!(step.format, Format::Pdf | Format::ImagePdf) {
                return Err("PDF optimization is available only for PDF or image-only PDF saving steps".into());
            }
            if !step.dpi.is_finite() || !(18.0..=1200.0).contains(&step.dpi) || !(1..=100).contains(&step.jpeg_quality) {
                return Err("output resolution must be 18..1200 dpi and JPEG quality 1..100".into());
            }
            validate_template(&step.name_template)?;
            if step.language.is_empty() || step.language.len() > 63 || !step.language.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                return Err("EPUB language must be a language tag such as de-AT or und".into());
            }
        }
        Ok(())
    }
}

/// An already processed PDF and its source-relative path; pages stay in this input order.
#[derive(Clone, Debug)]
pub struct Input {
    pub relative: PathBuf,
    pub bytes: Arc<Vec<u8>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OutputReceipt {
    pub outputs: Vec<PathBuf>,
    pub proofs: Vec<OutputProof>,
    /// Rendered pages that were bounded by the renderer's pixel limit.
    pub warnings: Vec<String>,
    #[serde(default)]
    pub optimizations: Vec<optimization::Report>,
}

/// Expected content computed from staged export bytes, before publication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputProof {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputError {
    pub message: String,
    /// A filesystem publication/staging failure can recover without changing the input.
    #[serde(default)]
    pub retryable: bool,
    /// Complete files already published (or reused after byte-identical comparison).
    /// No source action is permitted after this error, even when this list is nonempty.
    pub outputs: Vec<PathBuf>,
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}; all originals retained", self.message)
    }
}
impl std::error::Error for OutputError {}

/// Blocking worker API. An error always keeps sources untouched. Original-file actions
/// belong to the scheduler only after it durably journals the complete successful receipt.
pub fn save(
    inputs: &[Input],
    default_output: &Path,
    plan: &OutputPlan,
    task_name: &str,
    batch_time: u64,
    root: Option<&Path>,
    should_stop: &dyn Fn() -> bool,
) -> Result<OutputReceipt, OutputError> {
    let mut receipt = OutputReceipt::default();
    let mut retryable = false;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        save_inner(inputs, default_output, plan, task_name, batch_time, root, should_stop, &mut receipt, &mut retryable)
    }));
    match result {
        Ok(Ok(())) => Ok(receipt),
        Ok(Err(message)) => Err(OutputError { message, outputs: receipt.outputs, retryable }),
        Err(_) => Err(OutputError { message: "output conversion failed unexpectedly".into(), outputs: receipt.outputs, retryable: false }),
    }
}

struct Group<'a> {
    inputs: Vec<&'a Input>,
    subfolder: PathBuf,
    name: String,
    folder_name: String,
}

fn relative_ok(relative: &Path) -> bool {
    !relative.as_os_str().is_empty() && relative.components().all(|part| matches!(part, Component::Normal(_)))
}

fn groups<'a>(inputs: &'a [Input], grouping: Grouping, task_name: &str) -> Vec<Group<'a>> {
    let folder_name = |parent: &Path| parent.file_name().map_or_else(|| task_name.to_owned(), |name| name.to_string_lossy().into_owned());
    match grouping {
        Grouping::File => inputs
            .iter()
            .map(|input| {
                let parent = input.relative.parent().unwrap_or(Path::new(""));
                Group {
                    inputs: vec![input],
                    subfolder: parent.to_owned(),
                    name: input.relative.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
                    folder_name: folder_name(parent),
                }
            })
            .collect(),
        Grouping::Folder => {
            let mut by_folder: BTreeMap<PathBuf, Vec<&Input>> = BTreeMap::new();
            for input in inputs {
                by_folder.entry(input.relative.parent().unwrap_or(Path::new("")).to_owned()).or_default().push(input);
            }
            by_folder
                .into_iter()
                .map(|(subfolder, inputs)| Group { name: folder_name(&subfolder), folder_name: folder_name(&subfolder), subfolder, inputs })
                .collect()
        }
        Grouping::All => {
            vec![Group { inputs: inputs.iter().collect(), subfolder: PathBuf::new(), name: task_name.into(), folder_name: task_name.into() }]
        }
    }
}

struct Staged {
    file: tempfile::NamedTempFile,
    folder: PathBuf,
    stem: String,
    extension: &'static str,
    size: u64,
    sha256: String,
    optimization: Option<optimization::Report>,
}

#[allow(clippy::too_many_arguments)]
fn save_inner(
    inputs: &[Input],
    default_output: &Path,
    plan: &OutputPlan,
    task_name: &str,
    batch_time: u64,
    root: Option<&Path>,
    should_stop: &dyn Fn() -> bool,
    receipt: &mut OutputReceipt,
    retryable: &mut bool,
) -> Result<(), String> {
    *retryable = true;
    plan.validate(default_output, root)?;
    *retryable = false;
    if inputs.is_empty() || inputs.len() > 10_000 || inputs.iter().any(|input| !relative_ok(&input.relative)) {
        return Err("output batch needs 1..10000 inputs with relative paths containing normal components only".into());
    }
    let mut total = 0_u64;
    for input in inputs {
        total = total.saturating_add(input.bytes.len() as u64);
        if total > MAX_INPUT_BYTES {
            return Err("combined output inputs exceed 512 MiB; use smaller batches".into());
        }
        if plan.steps.iter().any(|step| step.optimization.enabled()) {
            stopped(should_stop)?;
            optimization::reject_signed(input.bytes.clone())?;
        }
    }
    let timestamp = i64::try_from(batch_time).ok().and_then(|value| DateTime::<Utc>::from_timestamp(value, 0)).ok_or("invalid output timestamp")?;
    let mut staged = Vec::new();
    let mut staged_bytes = 0_u64;
    for step in &plan.steps {
        *retryable = true;
        let base = confined(step.folder.as_deref().unwrap_or(default_output), root)?;
        *retryable = false;
        for (index, group) in groups(inputs, step.grouping, task_name).into_iter().enumerate() {
            stopped(should_stop)?;
            *retryable = true;
            let folder = safe_subfolder(&base, &group.subfolder)?;
            *retryable = false;
            let stem = expand_name(&step.name_template, &group.name, &group.folder_name, task_name, timestamp, index.saturating_add(1))?;
            let bytes = if let [input] = group.inputs.as_slice() {
                input.bytes.clone()
            } else {
                let sources =
                    group.inputs.iter().map(|input| (input.relative.to_string_lossy().into_owned(), input.bytes.clone())).collect::<Vec<_>>();
                Session::new().with_timestamp(timestamp.timestamp()).combine(&sources).map_err(|error| error.to_string())?
            };
            if step.format == Format::Analysis {
                let report = crate::hot_folder_analysis::analyze(bytes.as_slice(), &[], should_stop)?;
                stage(&mut staged, &mut staged_bytes, &folder, &stem, "json", &report, retryable)?;
                continue;
            }
            let mut session = Session::new().with_timestamp(timestamp.timestamp());
            let id = session.open(format!("{stem}.pdf"), None, bytes.clone(), None).map_err(|error| error.to_string())?;
            let document = session.get(id).ok_or("output document unavailable")?;
            let pages = document.info.pages.len();
            if pages == 0 || pages > MAX_OUTPUT_FILES {
                return Err("output document needs 1..10000 pages".into());
            }
            let mut exporter = Exporter::new(document);
            if let Some(format) = step.format.image(step.jpeg_quality) {
                for page in 0..pages {
                    stopped(should_stop)?;
                    warn_resolution(&exporter, page, step.dpi, &stem, receipt);
                    let bytes = exporter.image(page, step.dpi, format)?;
                    stage(
                        &mut staged,
                        &mut staged_bytes,
                        &folder,
                        &format!("{stem}_page_{:04}", page.saturating_add(1)),
                        step.format.extension(),
                        &bytes,
                        retryable,
                    )?;
                }
            } else {
                let output = match step.format {
                    Format::Pdf => bytes,
                    Format::ImagePdf => {
                        let mut sources = Vec::new();
                        let mut images_size = 0_u64;
                        let mut prepared_size = 0_u64;
                        for page in 0..pages {
                            stopped(should_stop)?;
                            warn_resolution(&exporter, page, step.dpi, &stem, receipt);
                            let png = exporter.png(page, step.dpi)?;
                            images_size = images_size.saturating_add(png.len() as u64);
                            if images_size > MAX_INPUT_BYTES {
                                return Err("image-only PDF exceeds 512 MiB; lower the resolution".into());
                            }
                            let name = format!("page-{}.png", page.saturating_add(1));
                            let pdf = session
                                .create_from_images_with_resolution(
                                    &[(name.clone(), png)],
                                    crate::ImageResolution::Dpi(exporter.dpi_used(page, step.dpi)),
                                )
                                .map_err(|error| error.to_string())?;
                            prepared_size = prepared_size.saturating_add(pdf.len() as u64);
                            if prepared_size > MAX_INPUT_BYTES {
                                return Err("prepared image-only PDF exceeds 512 MiB; lower the resolution".into());
                            }
                            sources.push((format!("page-{}.pdf", page.saturating_add(1)), pdf));
                        }
                        session.combine(&sources).map_err(|error| error.to_string())?
                    }
                    Format::Txt => Arc::new(exporter.text_of(&(0..pages).collect::<Vec<_>>())?.into_bytes()),
                    Format::Docx => Arc::new(document.export_office(crate::compare::OfficeFormat::Docx)),
                    Format::Html => Arc::new(document.export_office(crate::compare::OfficeFormat::Html)),
                    Format::Rtf => Arc::new(document.export_office(crate::compare::OfficeFormat::Rtf)),
                    Format::Xlsx => Arc::new(pdfcraft_export::xlsx(&document.export_pages(), &stem)?),
                    Format::Epub => Arc::new(pdfcraft_export::epub(&document.export_pages(), &stem, &step.language)?),
                    Format::Png | Format::Jpeg | Format::Tiff | Format::Analysis => return Err("invalid output state".into()),
                };
                let (output, optimized) = if matches!(step.format, Format::Pdf | Format::ImagePdf) {
                    step.optimization.apply(output, timestamp.timestamp(), should_stop)?
                } else {
                    (output, None)
                };
                stage(&mut staged, &mut staged_bytes, &folder, &stem, step.format.extension(), output.as_slice(), retryable)?;
                if let Some(file) = staged.last_mut() {
                    file.optimization = optimized;
                }
            }
        }
    }
    // All render/export work succeeded; only now publish the staged files.
    for mut file in staged {
        stopped(should_stop)?;
        let bytes = file.size;
        let sha256 = file.sha256.clone();
        let optimized = file.optimization.take();
        *retryable = true;
        let path = publish(file, root)?;
        *retryable = false;
        receipt.proofs.push(OutputProof { path: path.clone(), bytes, sha256 });
        if let Some(mut report) = optimized {
            report.path = path.clone();
            receipt.optimizations.push(report);
        }
        receipt.outputs.push(path);
    }
    Ok(())
}

fn warn_resolution(exporter: &Exporter, page: usize, dpi: f64, name: &str, receipt: &mut OutputReceipt) {
    let actual = exporter.dpi_used(page, dpi);
    if actual + 0.1 < dpi {
        receipt.warnings.push(format!(
            "{name}, page {}: rendered at {actual:.1} dpi instead of {dpi:.1} because of the page pixel limit",
            page.saturating_add(1)
        ));
    }
}

fn stopped(should_stop: &dyn Fn() -> bool) -> Result<(), String> {
    if should_stop() { Err("output task stopped".into()) } else { Ok(()) }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn stage(
    staged: &mut Vec<Staged>,
    total: &mut u64,
    folder: &Path,
    stem: &str,
    extension: &'static str,
    bytes: &[u8],
    retryable: &mut bool,
) -> Result<(), String> {
    *total = total.saturating_add(bytes.len() as u64);
    if bytes.is_empty() || *total > MAX_STAGED_BYTES || staged.len() >= MAX_OUTPUT_FILES {
        return Err("output batch exceeds 1 GiB or 10000 files, or an exporter returned an empty file; use smaller batches".into());
    }
    *retryable = true;
    let mut file = tempfile::Builder::new().prefix(".pdfcraft-output-").tempfile_in(folder).map_err(|error| error.to_string())?;
    file.write_all(bytes).and_then(|_| file.as_file().sync_all()).map_err(|error| format!("cannot stage output: {error}"))?;
    *retryable = false;
    staged.push(Staged {
        file,
        folder: folder.to_owned(),
        stem: stem.to_owned(),
        extension,
        size: bytes.len() as u64,
        sha256: digest(bytes),
        optimization: None,
    });
    Ok(())
}

fn publish(mut file: Staged, root: Option<&Path>) -> Result<PathBuf, String> {
    if confined(&file.folder, root)? != file.folder {
        return Err("output directory changed during conversion".into());
    }
    for counter in 0..10_000 {
        let name = if counter == 0 { format!("{}.{}", file.stem, file.extension) } else { format!("{} ({counter}).{}", file.stem, file.extension) };
        let target = file.folder.join(name);
        // Identical complete outputs are reusable on retry; different data always gets a
        // suffix. Links/directories are never followed or overwritten.
        if same_file(&target, file.size, &file.sha256)? {
            crate::hot_folder::sync_parent(&target)?;
            return Ok(target);
        }
        // Existing links or directories occupy a name too. Windows may report
        // AccessDenied rather than AlreadyExists when persisting over a directory.
        match fs::symlink_metadata(&target) {
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect output filename: {error}")),
        }
        match file.file.persist_noclobber(&target) {
            Ok(_) => {
                crate::hot_folder::sync_parent(&target)?;
                return Ok(target);
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => file.file = error.file,
            Err(error) => return Err(format!("cannot publish {}: {error}", target.display())),
        }
    }
    Err("10000 output filenames already exist; no file was overwritten".into())
}

fn same_file(path: &Path, size: u64, hash: &str) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("cannot inspect output: {error}")),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != size {
        return Ok(false);
    }
    let mut reader = File::open(path).map_err(|error| error.to_string())?.take(size.saturating_add(1));
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        let count = reader.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(buffer.get(..count).ok_or("invalid output read size")?);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>() == hash)
}

fn safe_subfolder(base: &Path, relative: &Path) -> Result<PathBuf, String> {
    let mut path = base.to_owned();
    for part in relative.components() {
        if !matches!(part, Component::Normal(_)) {
            return Err("invalid output hierarchy".into());
        }
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("output hierarchy contains a link or a file".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&path).map_err(|error| error.to_string())?;
                crate::hot_folder::sync_parent(&path)?;
            }
            Err(error) => return Err(error.to_string()),
        }
        if !path.canonicalize().map_err(|error| error.to_string())?.starts_with(base) {
            return Err("output hierarchy escaped its folder".into());
        }
    }
    path.canonicalize().map_err(|error| error.to_string())
}

const TOKENS: &[&str] = &["name", "folder", "task", "date", "time", "counter"];
fn forbidden_name_char(c: char) -> bool {
    c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
}

fn validate_template(template: &str) -> Result<(), String> {
    if template.trim().is_empty() || template.len() > 200 || template.chars().any(forbidden_name_char) {
        return Err("output filename must be 1..200 bytes without path separators or reserved filename characters".into());
    }
    let mut remaining = template;
    while let Some(open) = remaining.find('{') {
        let before = remaining.get(..open).ok_or("invalid filename template")?;
        if before.contains('}') {
            return Err("unmatched '}' in output filename".into());
        }
        let rest = remaining.get(open.saturating_add(1)..).ok_or("invalid filename template")?;
        let end = rest.find('}').ok_or("unmatched '{' in output filename")?;
        let token = rest.get(..end).ok_or("invalid filename token")?;
        if !TOKENS.contains(&token) {
            return Err(format!("unknown filename token {{{token}}}; use name, folder, task, date, time or counter"));
        }
        remaining = rest.get(end.saturating_add(1)..).ok_or("invalid filename template")?;
    }
    if remaining.contains('}') {
        return Err("unmatched '}' in output filename".into());
    }
    Ok(())
}

fn safe_component(value: &str) -> String {
    let value: String = value.chars().map(|c| if forbidden_name_char(c) || matches!(c, '{' | '}') { '_' } else { c }).collect();
    let value = value.trim_matches([' ', '.']);
    if value.is_empty() {
        "output".into()
    } else {
        let mut component = String::new();
        for character in value.chars() {
            if component.len().saturating_add(character.len_utf8()) > 180 {
                break;
            }
            component.push(character);
        }
        component
    }
}

fn expand_name(template: &str, name: &str, folder: &str, task: &str, timestamp: DateTime<Utc>, counter: usize) -> Result<String, String> {
    validate_template(template)?;
    let mut name = template
        .replace("{name}", &safe_component(name))
        .replace("{folder}", &safe_component(folder))
        .replace("{task}", &safe_component(task))
        .replace("{date}", &timestamp.format("%Y%m%d").to_string())
        .replace("{time}", &timestamp.format("%H%M%S").to_string())
        .replace("{counter}", &format!("{counter:04}"));
    name = safe_component(&name);
    let reserved = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    if matches!(
        reserved.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$" | "COM¹" | "COM²" | "COM³" | "LPT¹" | "LPT²" | "LPT³"
    ) || (1..=9).any(|n| reserved == format!("COM{n}") || reserved == format!("LPT{n}"))
    {
        name.insert(0, '_');
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, text: &str) -> Input {
        Input { relative: name.into(), bytes: Session::new().create_from_text(name, text).unwrap() }
    }
    fn run(inputs: &[Input], folder: &Path, steps: Vec<SaveStep>) -> Result<OutputReceipt, OutputError> {
        save(inputs, folder, &OutputPlan { steps }, "Batch", 1_700_000_000, None, &|| false)
    }
    fn text(path: &Path) -> String {
        let mut session = Session::new();
        let id = session.open("result.pdf", None, Arc::new(fs::read(path).unwrap()), None).unwrap();
        let doc = session.get(id).unwrap();
        Exporter::new(doc).text_of(&(0..doc.info.pages.len()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn groups_by_file_folder_and_all_and_preserves_page_order() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = [input("one/a.pdf", "Alpha first"), input("one/b.pdf", "Beta second"), input("two/c.pdf", "Gamma third")];
        let steps = vec![
            SaveStep::default(),
            SaveStep { grouping: Grouping::Folder, name_template: "folder-{folder}".into(), ..Default::default() },
            SaveStep { grouping: Grouping::All, name_template: "all-{task}".into(), ..Default::default() },
        ];
        let receipt = run(&inputs, dir.path(), steps).unwrap();
        assert_eq!(receipt.outputs.len(), 6);
        assert!(dir.path().join("one/a.pdf").is_file());
        let folder = text(&dir.path().join("one/folder-one.pdf"));
        assert!(folder.find("Alpha first").unwrap() < folder.find("Beta second").unwrap());
        let all = text(&dir.path().join("all-Batch.pdf"));
        assert!(all.find("Beta second").unwrap() < all.find("Gamma third").unwrap());
        assert!(!folder.contains("Gamma third"));
    }

    #[test]
    fn saves_multiple_real_export_formats_and_an_image_only_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let steps = Format::ALL
            .iter()
            .map(|format| SaveStep {
                format: *format,
                name_template: if *format == Format::ImagePdf { "image-only".into() } else { "result".into() },
                dpi: 72.0,
                ..Default::default()
            })
            .collect();
        let receipt = run(&[input("source.pdf", "Text survives editable export")], dir.path(), steps).unwrap();
        assert_eq!(receipt.outputs.len(), Format::ALL.len());
        assert!(fs::read_to_string(dir.path().join("result.txt")).unwrap().contains("Text survives"));
        assert!(fs::read_to_string(dir.path().join("result.html")).unwrap().contains("Text survives"));
        assert!(fs::read(dir.path().join("result.docx")).unwrap().starts_with(b"PK\x03\x04"));
        for extension in ["xlsx", "epub"] {
            assert!(fs::read(dir.path().join(format!("result.{extension}"))).unwrap().starts_with(b"PK\x03\x04"));
        }
        let analysis: serde_json::Value = serde_json::from_slice(&fs::read(dir.path().join("result.json")).unwrap()).unwrap();
        assert_eq!(analysis["format"], "pdfcraft-analysis");
        assert_eq!(analysis["pages"][0]["content"], "existing_text");
        assert!(fs::read_to_string(dir.path().join("result.rtf")).unwrap().starts_with("{\\rtf"));
        for extension in ["png", "jpg", "tif"] {
            let bytes = fs::read(dir.path().join(format!("result_page_0001.{extension}"))).unwrap();
            let image = image::load_from_memory(&bytes).unwrap();
            assert!(image.width() > 100 && image.height() > 100);
        }
        assert!(text(&dir.path().join("image-only.pdf")).trim().is_empty());
        assert!(text(&dir.path().join("result.pdf")).contains("Text survives"));
        // Raster-only export retains document geometry instead of defaulting to pixel=point.
        let mut session = Session::new();
        let id = session.open("image.pdf", None, Arc::new(fs::read(dir.path().join("image-only.pdf")).unwrap()), None).unwrap();
        let page = &session.get(id).unwrap().info.pages[0];
        assert!((page.width - 612.0).abs() < 1.0 && (page.height - 792.0).abs() < 1.0);
    }

    #[test]
    fn conversion_failure_never_publishes_the_successful_first_step() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = [input("valid.pdf", "Keep me"), Input { relative: "broken.pdf".into(), bytes: Arc::new(b"broken pdf".to_vec()) }];
        let error = run(&inputs, dir.path(), vec![SaveStep::default()]).unwrap_err();
        assert!(error.outputs.is_empty());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        assert_eq!(*inputs[1].bytes, b"broken pdf");
    }

    #[test]
    fn occupied_names_are_never_overwritten_and_identical_retry_reuses_results() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("source.pdf"), b"unrelated user document").unwrap();
        let inputs = [input("source.pdf", "Keep recognised text")];
        let first = run(&inputs, dir.path(), vec![SaveStep::default()]).unwrap();
        assert!(first.outputs[0].ends_with("source (1).pdf"));
        let second = run(&inputs, dir.path(), vec![SaveStep::default()]).unwrap();
        assert_eq!(first.outputs, second.outputs);
        assert_eq!(fs::read(dir.path().join("source.pdf")).unwrap(), b"unrelated user document");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        fs::create_dir(dir.path().join("folder.pdf")).unwrap();
        let next = run(&[input("folder.pdf", "kept separate")], dir.path(), vec![SaveStep::default()]).unwrap();
        assert!(next.outputs[0].ends_with("folder (1).pdf"));
        assert!(dir.path().join("folder.pdf").is_dir());
    }

    #[test]
    fn template_and_path_boundaries_reject_escapes_and_use_batch_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        for template in ["../escape", "C:\\escape", "{unknown}", "bad{name", "bad}name", "a/b", "{name}:test"] {
            assert!(run(&[input("good.pdf", "text")], dir.path(), vec![SaveStep { name_template: template.into(), ..Default::default() }]).is_err());
        }
        for relative in ["../escape.pdf", "/outside.pdf", "child/../../escape.pdf"] {
            assert!(run(&[input(relative, "text")], dir.path(), vec![SaveStep::default()]).is_err());
        }
        let result = run(
            &[input("x.pdf", "text")],
            dir.path(),
            vec![SaveStep { name_template: "{task}-{date}-{time}-{counter}-{name}".into(), ..Default::default() }],
        )
        .unwrap();
        assert!(result.outputs[0].ends_with("Batch-20231114-221320-0001-x.pdf"));
        let result = run(&[input("CON.pdf", "text")], dir.path(), vec![SaveStep::default()]).unwrap();
        assert!(result.outputs[0].ends_with("_CON.pdf"));
        assert!(run(&[input("x.pdf", "text")], dir.path(), vec![SaveStep { dpi: f64::NAN, ..Default::default() }]).is_err());
    }

    #[test]
    fn stopped_publishing_reports_complete_outputs_and_keeps_the_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = [input("a.pdf", "first"), input("b.pdf", "second")];
        let error = save(&inputs, dir.path(), &OutputPlan::default(), "Batch", 0, None, &|| dir.path().join("a.pdf").exists()).unwrap_err();
        assert_eq!(error.outputs.len(), 1);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(text(&error.outputs[0]).contains("first"));
        assert!(inputs.iter().all(|input| !input.bytes.is_empty()));
    }

    #[test]
    fn root_and_subfolder_file_collisions_are_checked_before_publication() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let plan = OutputPlan { steps: vec![SaveStep { folder: Some(outside.path().to_owned()), ..Default::default() }] };
        assert!(save(&[input("a.pdf", "text")], root.path(), &plan, "Batch", 0, Some(root.path()), &|| false).is_err());
        fs::write(root.path().join("child"), b"must remain").unwrap();
        assert!(run(&[input("child/a.pdf", "text")], root.path(), vec![SaveStep::default()]).is_err());
        assert_eq!(fs::read(root.path().join("child")).unwrap(), b"must remain");
    }

    #[test]
    fn absent_plan_fields_keep_the_original_one_pdf_per_file_behavior() {
        let plan: OutputPlan = serde_json::from_str("{}").unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert!(!plan.needs_batch());
        assert_eq!(plan.steps[0].format, Format::Pdf);
        assert_eq!(plan.steps[0].name_template, "{name}");
        assert!(!plan.steps[0].optimization.enabled());
        for unsupported in ["djvu", "fb2", "csv"] {
            assert!(serde_json::from_str::<Format>(&format!("\"{unsupported}\"")).is_err());
        }
    }

    #[test]
    fn optimized_grouped_pdf_and_text_have_matching_proofs_sizes_and_stable_retries() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = [input("a.pdf", "Alpha first searchable page"), input("b.pdf", "Beta second searchable page")];
        let steps = vec![
            SaveStep {
                grouping: Grouping::All,
                name_template: "combined".into(),
                optimization: optimization::Settings { mode: optimization::Mode::Lossless, ..Default::default() },
                ..Default::default()
            },
            SaveStep { grouping: Grouping::All, name_template: "combined".into(), format: Format::Txt, ..Default::default() },
        ];
        fs::write(dir.path().join("combined.pdf"), b"unrelated existing file").unwrap();
        let receipt = run(&inputs, dir.path(), steps.clone()).unwrap();
        assert_eq!(receipt.outputs.len(), 2);
        assert_eq!(receipt.optimizations.len(), 1);
        let report = &receipt.optimizations[0];
        assert!(report.path.ends_with("combined (1).pdf"));
        assert!(report.bytes_after <= report.bytes_before);
        let bytes = fs::read(&report.path).unwrap();
        assert_eq!(bytes.len() as u64, report.bytes_after);
        let proof = receipt.proofs.iter().find(|proof| proof.path == report.path).unwrap();
        assert_eq!(proof.bytes, report.bytes_after);
        assert_eq!(proof.sha256, digest(&bytes));
        let contents = text(&report.path);
        assert!(contents.find("Alpha first").unwrap() < contents.find("Beta second").unwrap());
        assert!(fs::read_to_string(dir.path().join("combined.txt")).unwrap().contains("Beta second"));
        assert_eq!(run(&inputs, dir.path(), steps).unwrap().outputs, receipt.outputs);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
        assert_eq!(fs::read(dir.path().join("combined.pdf")).unwrap(), b"unrelated existing file");
    }

    #[test]
    fn custom_optimization_applies_after_image_only_export_and_rejects_non_pdf_steps() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = [input("source.pdf", "Raster output stays raster only")];
        let settings = optimization::Settings {
            mode: optimization::Mode::Custom,
            color: optimization::Images { downsample: false, compression: optimization::Compression::Zip, ..Default::default() },
            gray: optimization::Images { downsample: false, compression: optimization::Compression::Retain, ..Default::default() },
        };
        let step = SaveStep { format: Format::ImagePdf, dpi: 72.0, optimization: settings.clone(), ..Default::default() };
        let receipt = run(&inputs, dir.path(), vec![step]).unwrap();
        assert_eq!(receipt.optimizations.len(), 1);
        assert!(receipt.optimizations[0].bytes_after <= receipt.optimizations[0].bytes_before);
        assert!(text(&receipt.outputs[0]).trim().is_empty());
        let invalid = SaveStep { format: Format::Txt, optimization: settings, ..Default::default() };
        let error = run(&inputs, dir.path(), vec![invalid]).unwrap_err();
        assert!(error.message.contains("only for PDF"));
        assert!(error.outputs.is_empty());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn signed_input_is_refused_before_grouping_or_image_conversion_can_remove_its_signature() {
        let p12 = fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
        let identity = crate::sign::pkcs12::open(&p12, "test").unwrap();
        let mut session = Session::new().with_timestamp(1_800_000_000);
        let source = input("signed.pdf", "Keep the signature");
        let id = session.open("signed.pdf", None, source.bytes, None).unwrap();
        let signed = session.sign(id, &identity, crate::SignOptions { rect: None, ..Default::default() }).unwrap();
        for format in [Format::Pdf, Format::ImagePdf] {
            let dir = tempfile::tempdir().unwrap();
            let inputs = [input("first.pdf", "Unsigned first page"), Input { relative: "signed.pdf".into(), bytes: signed.clone() }];
            let steps = vec![SaveStep {
                format,
                grouping: Grouping::All,
                optimization: optimization::Settings { mode: optimization::Mode::Lossless, ..Default::default() },
                ..Default::default()
            }];
            let error = run(&inputs, dir.path(), steps).unwrap_err();
            assert!(error.message.contains("signed PDFs"), "{error}");
            assert!(error.outputs.is_empty());
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }
}
