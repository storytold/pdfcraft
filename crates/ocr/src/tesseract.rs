//! The Tesseract engine: an installed `tesseract` binary driven as an optional external
//! process (`tesseract <png> stdout -l <lang> tsv`), never linked, never bundled, never
//! downloaded. The whole module is compiled out for wasm32, where nothing may spawn.
//!
//! Security posture (hard rules): the program path comes from the PATH or a user-configured
//! location, never from document data; the language code is allow-listed before it reaches a
//! process; the page image goes to a private temp file (a fresh 0700 directory holding a 0600
//! file on Unix) that is deleted afterwards; the run has a timeout and an output size cap; and
//! every failure comes back through [`OcrError`] with its cause named.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use super::{Line, OcrError, OcrImage, RecognizeOptions, Recognizer, Word};

/// How long one recognition may run before the process is killed.
const TIMEOUT: Duration = Duration::from_secs(120);
/// How much TSV output is accepted before the run is abandoned.
const MAX_OUTPUT: u64 = 64 * 1024 * 1024;
/// How much of the program's stderr is read back (its end): enough for a `--list-langs` list
/// printed there, small enough that error text stays a message, not a memory DoS.
const MAX_STDERR: u64 = 256 * 1024;
/// The most words one TSV reading may hold. Known limit: a reading with more words than this
/// is truncated at the cap — corrupt or runaway output must not become unbounded memory in
/// the word list, the text layer and the review UI.
pub const MAX_WORDS: usize = 50_000;
/// Tesseract's code for each language PdfCraft offers (ISO 639-1 → tessdata name). Only these
/// codes ever reach the process, whatever the settings say.
pub const LANGUAGE_CODES: &[(&str, &str)] = &[
    ("en", "eng"),
    ("de", "deu"),
    ("fr", "fra"),
    ("es", "spa"),
    ("it", "ita"),
    ("pt", "por"),
    ("ru", "rus"),
    ("zh", "chi_sim"),
    ("ja", "jpn"),
    ("ko", "kor"),
    ("ar", "ara"),
    ("nl", "nld"),
];

/// English names for [`LANGUAGE_CODES`] (dropdowns show code + name).
pub const LANGUAGE_NAMES: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("ru", "Russian"),
    ("zh", "Chinese (Simplified)"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("ar", "Arabic"),
    ("nl", "Dutch"),
];

/// Map a settings language code to a Tesseract one: trimmed, case-insensitive, allow-listed;
/// anything unknown falls back to `"eng"`.
pub fn language_code(code: &str) -> &'static str {
    let c = code.trim().to_ascii_lowercase();
    LANGUAGE_CODES.iter().find(|(k, _)| *k == c).map(|(_, v)| *v).unwrap_or("eng")
}

/// The user-configured program location, when valid: `$PDFCRAFT_TESSERACT` (a user
/// preference, never taken from document data). An invalid value — relative, missing, a
/// directory, not executable — means unavailable: an explicit setting is never silently
/// replaced by a PATH search.
pub fn configured_path() -> Option<PathBuf> {
    let value = std::env::var_os("PDFCRAFT_TESSERACT")?;
    let program = PathBuf::from(value);
    program_is_executable(&program).then_some(program)
}

/// The program [`TesseractCli::find`] would run: a set `configured` location (the value of
/// `PDFCRAFT_TESSERACT`) decides alone, so an invalid one yields `None` and `search` (the PATH)
/// is not even consulted; only when nothing is configured does `search` decide.
fn locate(configured: Option<std::ffi::OsString>, search: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
    match configured {
        Some(value) => Some(PathBuf::from(value)).filter(|program| program_is_executable(program)),
        None => search(),
    }
}

/// The first `tesseract` on the PATH that is an existing executable file. Entries that are not
/// absolute are refused: a relative PATH entry resolves against the caller's working
/// directory, which is neither stable nor a safe place to find a program. Also named by
/// `ocr_status` (the engine reports where it would look, without probing).
pub fn path_search() -> Option<PathBuf> {
    path_search_in(std::env::split_paths(&std::env::var_os("PATH")?))
}

/// [`path_search`] over an explicit directory list (so tests can pass one).
fn path_search_in(dirs: impl Iterator<Item = PathBuf>) -> Option<PathBuf> {
    for dir in dirs.filter(|d| d.is_absolute()) {
        for name in program_file_names() {
            let candidate = dir.join(name);
            if program_is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// The file names an installed `tesseract` binary goes by.
fn program_file_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["tesseract.exe"]
    }
    #[cfg(not(windows))]
    {
        &["tesseract"]
    }
}

/// Whether `program` is a usable program location (see [`program_is_executable`]); callers
/// that run a validated program by path (the engine's `--version` query) check it first.
pub fn program_is_usable(program: &Path) -> bool {
    program_is_executable(program)
}

/// Whether `program` is a usable configured location: an absolute path to an existing,
/// executable FILE (on Unix the executable bits must be set; elsewhere being a file is what
/// the OS requires). A directory or a missing file is simply unavailable — never a panic.
fn program_is_executable(program: &Path) -> bool {
    if !program.is_absolute() || !program.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(program).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// One row of Tesseract's TSV output. `level` 5 rows are the words; `conf` is −1 on the header
/// rows (page, block, paragraph, line), which are not words.
struct TsvRow<'a> {
    level: &'a str,
    block: u32,
    par: u32,
    line: u32,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    conf: f32,
    text: &'a str,
}

/// Parse one TSV row; `None` for anything unreadable (short rows, non-numeric fields), which
/// is skipped rather than trusted. A row with non-finite numbers or a negative width or height
/// is not a word either: geometry like that is output corruption, and trusting it puts NaN
/// into word boxes and on to the page.
fn tsv_row(row: &str) -> Option<TsvRow<'_>> {
    let c: Vec<&str> = row.split('\t').collect();
    if c.len() < 12 {
        return None;
    }
    let (block, par, line) = (c[2].trim().parse().ok()?, c[3].trim().parse().ok()?, c[4].trim().parse().ok()?);
    let (left, top): (f32, f32) = (c[6].trim().parse().ok()?, c[7].trim().parse().ok()?);
    let (width, height, conf): (f32, f32, f32) = (c[8].trim().parse().ok()?, c[9].trim().parse().ok()?, c[10].trim().parse().ok()?);
    if !left.is_finite() || !top.is_finite() || !width.is_finite() || !height.is_finite() || !conf.is_finite() {
        return None; // NaN/inf anywhere in the numbers
    }
    if width < 0.0 || height < 0.0 {
        return None; // a word box has a non-negative size
    }
    Some(TsvRow { level: c[0], block, par, line, left, top, width, height, conf, text: c[11] })
}

/// Parse `tesseract … tsv` output into lines of words. Rows with confidence −1 are the
/// structural header rows and never become words; consecutive word rows that share their
/// (block, paragraph, line) form one [`Line`], in output (reading) order. Confidence clamps
/// into 0–100. Known limit: past [`MAX_WORDS`] the rest of the reading is dropped (truncated,
/// not an error — the words before the cap are still placed).
pub fn parse_tsv(tsv: &str) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut key: Option<(u32, u32, u32)> = None;
    let mut words = 0usize;
    for row in tsv.lines() {
        if words >= MAX_WORDS {
            break; // the reading is longer than the cap: the rest is dropped
        }
        let Some(r) = tsv_row(row) else { continue };
        if r.level != "5" || r.conf < 0.0 || r.text.trim().is_empty() {
            continue; // headers (conf −1), non-word levels, and blank output are not words
        }
        let this = (r.block, r.par, r.line);
        if key != Some(this) {
            lines.push(Line::default());
            key = Some(this);
        }
        let word = Word::new(r.text, [r.left, r.top, r.left + r.width, r.top + r.height], "tesseract").with_confidence(r.conf.clamp(0.0, 100.0));
        if let Some(last) = lines.last_mut() {
            last.words.push(word);
            words += 1;
        }
    }
    lines
}

/// Parse `tesseract --list-langs` output: the header line ("List of available languages…"),
/// then one code per line. Lines with spaces are not codes (a header, a warning) and are left
/// out.
pub fn parse_list_langs(out: &str) -> Vec<String> {
    out.lines()
        .skip_while(|l| !l.trim().is_empty() && l.trim().starts_with("List of available"))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.contains(char::is_whitespace))
        .collect()
}

/// The languages a `--list-langs` run reported: its stdout, or, when that holds none, its
/// stderr (Tesseract 3.x prints the list there).
pub fn list_langs_from(stdout: &str, stderr: &str) -> Vec<String> {
    let codes = parse_list_langs(stdout);
    if codes.is_empty() { parse_list_langs(stderr) } else { codes }
}

/// The first non-empty line of what `tesseract --version` printed: on stdout, or on stderr
/// where older versions print it.
fn version_line(stdout: &str, stderr: &str) -> Option<String> {
    let first = |text: &str| text.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_owned);
    first(stdout).or_else(|| first(stderr))
}

/// The Tesseract program. Build one (via [`TesseractCli::find`] for the PATH copy, or
/// [`TesseractCli::new`] for a user-configured path) and reuse it: `--list-langs` runs at most
/// once.
pub struct TesseractCli {
    program: PathBuf,
    /// The timeout and the output cap are fields so tests can shorten them.
    timeout: Duration,
    max_output: u64,
    /// Whether `--list-langs` has succeeded yet: 0 untried, 1 yes, 2 no.
    probed: AtomicU8,
    langs: Mutex<Vec<(String, String)>>,
}

impl TesseractCli {
    /// The program at `path` (a user preference; never taken from document data).
    pub fn new(program: PathBuf) -> TesseractCli {
        TesseractCli { program, timeout: TIMEOUT, max_output: MAX_OUTPUT, probed: AtomicU8::new(0), langs: Mutex::new(Vec::new()) }
    }

    /// The program this engine runs (`ocr_status` names it).
    pub fn program(&self) -> &std::path::Path {
        &self.program
    }

    /// The program at a user-configured `path`, if it is usable there: only an absolute path
    /// to an existing, executable file is accepted (a directory, a missing file or a relative
    /// path is unavailable, never a panic), and it must answer `--list-langs`.
    pub fn at(program: PathBuf) -> Option<TesseractCli> {
        if !program_is_executable(&program) {
            return None;
        }
        let t = TesseractCli::new(program);
        let langs = t.list_langs().ok()?;
        *t.langs.lock().unwrap_or_else(|e| e.into_inner()) = langs;
        t.probed.store(1, Ordering::Relaxed);
        Some(t)
    }

    /// The program this machine offers, if it answers `--list-langs`: the user-configured
    /// location ([`configured_path`] / `$PDFCRAFT_TESSERACT`) when valid, else the first
    /// absolute PATH entry that holds an executable `tesseract`.
    ///
    /// # The PATH-lookup risk
    ///
    /// Resolving through the PATH trusts every directory on it: a binary planted earlier on
    /// the PATH runs as a subprocess with this application's privileges — it sees the page
    /// image and anything the process can reach. The language code and the page image still
    /// never come from document data, but a user who handles untrusted documents should pin
    /// the exact program with `PDFCRAFT_TESSERACT`, which is validated (absolute, existing,
    /// executable file) before any process starts.
    ///
    /// A `PDFCRAFT_TESSERACT` that is set but invalid fails closed: it is never replaced by
    /// the PATH search.
    pub fn find() -> Option<TesseractCli> {
        TesseractCli::at(locate(std::env::var_os("PDFCRAFT_TESSERACT"), path_search)?)
    }

    /// Run `tesseract --list-langs` and parse it; also decides [`TesseractCli::available`].
    fn list_langs(&self) -> Result<Vec<(String, String)>, OcrError> {
        let (out, err) = self.run_capture(&["--list-langs"], self.timeout, self.max_output)?;
        let codes = list_langs_from(&out, &err);
        if codes.is_empty() {
            return Err(OcrError::NoTesseract("--list-langs reported no languages".into()));
        }
        Ok(codes
            .into_iter()
            .map(|c| {
                let name = LANGUAGE_NAMES.iter().find(|(k, _)| *k == c.as_str()).map(|(_, n)| (*n).to_string()).unwrap_or_else(|| c.clone());
                (c, name)
            })
            .collect())
    }

    /// The program's first `--version` line (named by `ocr_status`); `None` when the program
    /// does not answer. Not cached: status asks rarely, and a re-install mid-session should
    /// show up without a restart.
    pub fn version(&self) -> Option<String> {
        let (out, err) = self.run_capture(&["--version"], self.timeout, self.max_output).ok()?;
        version_line(&out, &err)
    }

    /// Run the program with fixed argv, its stdout in a private temp file, under `timeout`
    /// seconds and an output cap of `max_output` bytes. Returns the (capped) stdout.
    fn run(&self, args: &[&str], timeout: Duration, max_output: u64) -> Result<String, OcrError> {
        self.run_capture(args, timeout, max_output).map(|(out, _)| out)
    }

    /// [`Self::run`], also returning what the program wrote to stderr (up to
    /// [`MAX_STDERR`] bytes, its end kept): some Tesseract versions print the `--list-langs`
    /// list there instead of on stdout.
    fn run_capture(&self, args: &[&str], timeout: Duration, max_output: u64) -> Result<(String, String), OcrError> {
        let dir = TempDir::create()?;
        let out_path = dir.path.join("out.txt");
        let err_path = dir.path.join("err.txt");
        let out_file = std::fs::File::create(&out_path).map_err(|e| OcrError::Process(format!("creating the output file failed: {e}")))?;
        // stderr goes to a file, not a pipe: an undrained pipe fills (around 64 KiB) and then
        // blocks the child mid-write, which a watcher can only read as a timeout. A file never
        // blocks; it is read back as a short tail after the run.
        let err_file = std::fs::File::create(&err_path).map_err(|e| OcrError::Process(format!("creating the error file failed: {e}")))?;
        // Known limit: only the direct child is killed at the timeout or on an early cap. A
        // wrapper script's children (grandchildren of this process) may survive the kill; the
        // cap and the timeout still bound what this run reads and how long it waits.
        let mut child =
            std::process::Command::new(&self.program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(out_file)
                .stderr(err_file)
                .current_dir(&dir.path)
                // A minimal environment: nothing from the document or the caller's shell beyond
                // what an installed tesseract needs — its data prefix, the PATH (bare-name lookup
                // and DLL loading), the Windows system root, and a temp dir it may write to.
                .env_clear()
                .envs(std::env::vars().filter(|(k, _)| {
                    k.eq_ignore_ascii_case("TESSDATA_PREFIX") || k.eq_ignore_ascii_case("PATH") || k.eq_ignore_ascii_case("SystemRoot")
                }))
                .env("TMP", &dir.path)
                .env("TEMP", &dir.path)
                .spawn()
                .map_err(|e| OcrError::NoTesseract(format!("{} could not start: {e}", self.program.display())))?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {
                    // The output caps are enforced while the child runs: a flood is killed as
                    // soon as a file passes its cap, not after it has finished writing. The
                    // read-back below caps what this run KEEPS of stderr; this cap bounds what
                    // the child can write there in the first place.
                    if std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0) > max_output
                        || std::fs::metadata(&err_path).map(|m| m.len()).unwrap_or(0) > MAX_STDERR
                    {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(OcrError::OutputTooLarge(max_output));
                    }
                    if started.elapsed() >= timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(OcrError::Timeout(timeout.as_secs()));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(OcrError::Process(format!("waiting for {} failed: {e}", self.program.display())));
                }
            }
        };
        let status = status?;
        // Whatever the exit, the stderr tail names the cause (capped: error text stays a
        // message, not a memory DoS).
        let stderr = std::fs::File::open(&err_path)
            .and_then(|mut f| {
                use std::io::{Read, Seek, SeekFrom};
                let len = f.metadata()?.len().min(MAX_STDERR);
                f.seek(SeekFrom::End(-(len as i64)))?;
                let mut buf = Vec::with_capacity(len as usize);
                f.take(len).read_to_end(&mut buf)?;
                Ok::<_, std::io::Error>(buf)
            })
            .unwrap_or_default();
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        // The message names only the end of it.
        let tail = stderr.char_indices().rev().nth(2047).map_or(0, |(i, _)| i);
        let note = stderr.get(tail..).unwrap_or_default().trim();
        if !status.success() {
            return Err(OcrError::Process(format!(
                "{} exited with {}: {}",
                self.program.display(),
                status.code().map(|c| c.to_string()).unwrap_or_else(|| "a signal".into()),
                if note.is_empty() { "no error message" } else { note }
            )));
        }
        let len = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
        if len > max_output {
            return Err(OcrError::OutputTooLarge(max_output));
        }
        let bytes = std::fs::read(&out_path).map_err(|e| OcrError::Process(format!("reading the output failed: {e}")))?;
        Ok((String::from_utf8_lossy(&bytes).into_owned(), stderr))
    }

    /// Retry `--list-langs` once (an install may have appeared since the last try).
    fn refresh(&self) -> Result<(), OcrError> {
        let langs = self.list_langs()?;
        *self.langs.lock().unwrap_or_else(|e| e.into_inner()) = langs;
        self.probed.store(1, Ordering::Relaxed);
        Ok(())
    }
}

impl Recognizer for TesseractCli {
    fn id(&self) -> &'static str {
        "tesseract"
    }

    /// Whether the program answers `--list-langs` (remembered; one process at most after the
    /// first call).
    fn available(&self) -> bool {
        match self.probed.load(Ordering::Relaxed) {
            1 => true,
            2 => false,
            _ => self.refresh().is_ok(),
        }
    }

    fn languages(&self) -> Vec<(String, String)> {
        if self.probed.load(Ordering::Relaxed) == 2 {
            return Vec::new();
        }
        let langs = self.langs.lock().unwrap_or_else(|e| e.into_inner());
        if langs.is_empty() {
            drop(langs);
            return self.refresh().map_or(Vec::new(), |()| self.langs.lock().unwrap_or_else(|e| e.into_inner()).clone());
        }
        langs.clone()
    }

    /// Write the page as a PNG and read it back as TSV.
    fn recognize(&self, image: &OcrImage, options: &RecognizeOptions) -> Result<Vec<Line>, OcrError> {
        let lang = language_code(&options.language);
        let png = encode_png(image)?;
        let dir = TempDir::create()?;
        let png_path = dir.path.join("page.png");
        {
            let mut f = std::fs::File::create(&png_path).map_err(|e| OcrError::Process(format!("creating the image file failed: {e}")))?;
            set_private(&mut f);
            f.write_all(&png).map_err(|e| OcrError::Process(format!("writing the image failed: {e}")))?;
        }
        // Fixed argv, no shell: tesseract <png> stdout -l <lang> tsv.
        let tsv = self.run(&[&png_path.to_string_lossy(), "stdout", "-l", lang, "tsv"], self.timeout, self.max_output)?;
        Ok(parse_tsv(&tsv))
    }
}

/// Encode an RGBA raster as PNG (the interchange format Tesseract reads best).
fn encode_png(image: &OcrImage) -> Result<Vec<u8>, OcrError> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, image.width, image.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| OcrError::Process(format!("encoding the page image failed: {e}")))?;
    writer.write_image_data(&image.rgba).map_err(|e| OcrError::Process(format!("encoding the page image failed: {e}")))?;
    writer.finish().map_err(|e| OcrError::Process(format!("encoding the page image failed: {e}")))?;
    Ok(out)
}

/// Make a just-created file owner-only, where the OS supports it (Unix). Windows relies on the
/// user's private %TEMP% ACLs; the fresh directory keeps other sessions' files out of sight.
fn set_private(f: &mut std::fs::File) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = f;
}

/// A fresh directory under the system temp dir, deleted (with its contents) when dropped.
/// On Unix it is created 0700, so the page image inside is private.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn create() -> Result<TempDir, OcrError> {
        // The recognizer is shared across threads (`Arc`), so two `recognize` calls can run at
        // once, and on a coarse clock they can read the same nanosecond. One counter value per
        // attempt keeps the names of one process apart without trusting the clock.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        // Created owner-only in one step on Unix (no window in which the directory exists with
        // the default, wider mode); `create` refuses a path that already exists.
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // Elsewhere the default mode stands (Windows relies on the user's private %TEMP% ACLs).
        #[cfg(not(unix))]
        let _ = &mut builder;
        // A leftover directory of an earlier process with the same pid can still take a name;
        // that one collision is retried with the next counter value, anything else is fatal.
        for _ in 0..4 {
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("pdfcraft-ocr-{}-{nanos}-{seq}", std::process::id()));
            match builder.create(&path) {
                Ok(()) => return Ok(TempDir { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(OcrError::Process(format!("creating the private temp directory failed: {e}"))),
            }
        }
        Err(OcrError::Process("creating the private temp directory failed: every candidate name already exists".into()))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The Unix process tests below build fake scripts and name their paths.
    #[cfg(unix)]
    use std::path::Path;

    /// Several directories alive at once never share a name: the recognizer is shared across
    /// threads (`Arc`), so two `recognize` calls can create their directories concurrently,
    /// and the counter in the name — not the clock alone — tells them apart.
    #[test]
    fn temp_directories_alive_at_once_have_distinct_names() {
        let dirs: Vec<TempDir> = (0..32).map(|_| TempDir::create().expect("create")).collect();
        let distinct: std::collections::BTreeSet<std::path::PathBuf> = dirs.iter().map(|d| d.path.clone()).collect();
        assert_eq!(distinct.len(), dirs.len(), "{:?}", dirs.iter().map(|d| &d.path).collect::<Vec<_>>());
    }

    /// A tiny image with one dark square (a synthetic "word"); the Unix process tests use it.
    #[cfg(unix)]
    fn blot(w: u32, h: u32, x0: u32, y0: u32, bw: u32, bh: u32) -> OcrImage {
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        for y in y0..(y0 + bh).min(h) {
            for x in x0..(x0 + bw).min(w) {
                let o = ((y * w + x) * 4) as usize;
                rgba[o..o + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        OcrImage::new(w, h, rgba).unwrap()
    }

    /// The literal TSV fixture: two header rows (conf −1) and two lines of words.
    const TSV: &str = "level\tpage\tblock\tpar\tline\tword\tleft\ttop\twidth\theight\tconf\ttext
1\t1\t0\t0\t0\t0\t0\t0\t100\t50\t-1\t
5\t1\t1\t1\t1\t1\t10\t10\t40\t12\t96.5\tHello
5\t1\t1\t1\t1\t2\t55\t10\t30\t12\t91.0\tworld
5\t1\t1\t2\t2\t1\t10\t30\t20\t12\t42.123\tsecond";

    #[test]
    fn tsv_parses_words_into_lines() {
        let lines = parse_tsv(TSV);
        assert_eq!(lines.len(), 2, "two (block, par, line) groups: {lines:?}");
        let first = &lines[0].words;
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].text, "Hello");
        assert_eq!(first[0].rect, [10.0, 10.0, 50.0, 22.0], "left + width, top + height");
        assert_eq!(first[0].confidence, Some(96.5));
        assert_eq!(first[0].source, "tesseract");
        assert_eq!(first[1].text, "world");
        assert_eq!(lines[1].words.len(), 1, "a new par starts a new line");
        assert_eq!(lines[1].words[0].text, "second");
        assert_eq!(lines[1].words[0].confidence, Some(42.123));
    }

    /// Conf −1 rows and level ≠ 5 rows are structure, not words; blank text is dropped.
    #[test]
    fn tsv_treats_header_rows_and_blank_text_as_not_words() {
        let tsv = "5\t1\t1\t1\t1\t1\t0\t0\t10\t10\t-1\tskipped\n\
                   4\t1\t1\t1\t1\t0\t0\t0\t10\t10\t95.0\tline row\n\
                   5\t1\t1\t1\t1\t1\t0\t0\t10\t10\t95.0\t   \n\
                   5\t1\t1\t1\t1\t2\t0\t0\t10\t10\t88\tkept";
        let lines = parse_tsv(tsv);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0].words.len(), 1);
        assert_eq!(lines[0].words[0].text, "kept");
    }

    /// Garbage rows (short, non-numeric) are skipped; CRLF endings survive; boxes saturate
    /// instead of overflowing.
    #[test]
    fn tsv_skips_garbage_and_never_panics() {
        let lines = parse_tsv("not a tsv row\n\n5\t1\t1\n5\t1\t1\t1\t1\t1\tx\ty\tw\th\tzzz\ttext\r\n");
        assert!(lines.is_empty(), "{lines:?}");
        let big = parse_tsv(&format!("5\t1\t1\t1\t1\t1\t{}\t0\t{}\t10\t50\tbig", u32::MAX, u32::MAX));
        let rect = big[0].words[0].rect;
        assert!(rect[2].is_finite() && rect[3].is_finite(), "no overflow: {rect:?}");
        // A row whose text field is empty yields no words.
        assert!(parse_tsv("5\t1\t1\t1\t1\t1\t1\t1\t2\t2\t50\t\t").is_empty());
    }

    /// Non-finite numbers and negative sizes are not words: a row with NaN/inf coordinates or
    /// a negative width/height never reaches the output (and so never reaches the content
    /// stream). `1e39` overflows f32 to infinity, so it is refused the same way.
    #[test]
    fn tsv_rejects_non_finite_and_negative_geometry() {
        let row = |left: &str, top: &str, w: &str, h: &str, conf: &str| format!("5\t1\t1\t1\t1\t1\t{left}\t{top}\t{w}\t{h}\t{conf}\tword");
        for (why, r) in [
            ("NaN left", row("NaN", "0", "10", "10", "50")),
            ("NaN width", row("0", "0", "NaN", "10", "50")),
            ("NaN height", row("0", "0", "10", "NaN", "50")),
            ("NaN confidence", row("0", "0", "10", "10", "NaN")),
            ("inf left", row("inf", "0", "10", "10", "50")),
            ("inf width", row("0", "0", "inf", "10", "50")),
            ("1e39 width overflows to inf", row("0", "0", "1e39", "10", "50")),
            ("negative width", row("0", "0", "-10", "10", "50")),
            ("negative height", row("0", "0", "10", "-10", "50")),
        ] {
            let lines = parse_tsv(&r);
            assert!(lines.is_empty(), "{why}: {lines:?}");
        }
        // A sane row still parses.
        assert_eq!(parse_tsv(&row("0", "0", "10", "10", "50"))[0].words[0].text, "word");
    }

    /// A reading longer than the word cap is truncated at [`MAX_WORDS`] (a known limit, not an
    /// error): the words before the cap are kept, the rest is dropped.
    #[test]
    fn tsv_truncates_readings_past_the_word_cap() {
        let mut tsv = String::new();
        for i in 0..MAX_WORDS + 5 {
            tsv.push_str(&format!("5\t1\t1\t1\t1\t{}\t{}\t10\t40\t12\t96.5\tw{i}\n", i % 7 + 1, i % 500));
        }
        let lines = parse_tsv(&tsv);
        let count: usize = lines.iter().map(|l| l.words.len()).sum();
        assert_eq!(count, MAX_WORDS, "truncated at the cap");
        assert_eq!(lines.last().and_then(|l| l.words.last()).map(|w| w.text.as_str()), Some(&*format!("w{}", MAX_WORDS - 1)));
    }

    #[test]
    fn language_table_maps_and_falls_back() {
        assert_eq!(language_code("en"), "eng");
        assert_eq!(language_code(" EN "), "eng", "trimmed, case-insensitive");
        assert_eq!(language_code("de"), "deu");
        assert_eq!(language_code("ZH"), "chi_sim");
        assert_eq!(language_code("ja"), "jpn");
        assert_eq!(language_code("ko"), "kor");
        assert_eq!(language_code("nl"), "nld");
        assert_eq!(language_code("klingon"), "eng", "unknown → eng");
        assert_eq!(language_code(""), "eng");
        // The security property: whatever the settings say, only allow-listed codes reach a
        // process.
        for code in ["en", "de", "fr", "es", "it", "pt", "ru", "zh", "ja", "ko", "ar", "nl", "klingon", "", " eng ", "EN/;rm"] {
            let mapped = language_code(code);
            assert!(LANGUAGE_CODES.iter().any(|(_, v)| *v == mapped), "{code:?} → {mapped:?} is not allow-listed");
        }
    }

    #[test]
    fn list_langs_come_from_stderr_when_stdout_has_none() {
        let listing = "List of available languages in \"/usr/share/tessdata/\" (2):\neng\nosd\n";
        assert_eq!(list_langs_from(listing, ""), ["eng", "osd"]);
        assert_eq!(list_langs_from("", listing), ["eng", "osd"], "3.x prints the list on stderr");
        assert_eq!(list_langs_from("", "Error opening data file\n"), Vec::<String>::new(), "a warning is not a language");
        assert_eq!(list_langs_from("eng\n", "deu\n"), ["eng"], "stdout wins when it has the list");
    }

    /// The private temp directory is created owner-only (Unix) and removed on drop.
    #[test]
    fn temp_dir_is_private_and_removed() {
        let dir = TempDir::create().unwrap();
        let path = dir.path.clone();
        assert!(path.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o700);
        }
        drop(dir);
        assert!(!path.exists());
    }

    /// A set `PDFCRAFT_TESSERACT` decides alone: when it is invalid, [`TesseractCli::find`] does
    /// not fall back to a PATH copy (which would run a program the user did not name); the PATH
    /// search only runs when nothing is configured.
    #[test]
    fn an_invalid_configured_program_fails_closed() {
        let Ok(real) = std::env::current_exe() else { return };
        let searched = std::cell::Cell::new(false);
        let on_path = || {
            searched.set(true);
            Some(real.clone())
        };
        assert_eq!(locate(None, on_path), Some(real.clone()), "nothing configured: the PATH search decides");
        assert!(searched.get());
        searched.set(false);
        assert_eq!(locate(Some(real.clone().into_os_string()), on_path), Some(real.clone()), "a valid location is used as is");
        assert!(!searched.get(), "and the PATH is not searched");
        let dir = real.parent().map(Path::to_path_buf).unwrap_or_default().into_os_string();
        for bad in [std::ffi::OsString::from("relative/tesseract"), std::ffi::OsString::from(""), dir, "/no/such/program-0123456789".into()] {
            assert_eq!(locate(Some(bad.clone()), on_path), None, "{bad:?} is invalid, so there is no program");
            assert!(!searched.get(), "{bad:?}: an invalid location never falls back to the PATH");
        }
    }

    #[test]
    fn the_version_comes_from_stdout_or_stderr() {
        assert_eq!(version_line("tesseract 5.4.0\n leptonica-1.84\n", ""), Some("tesseract 5.4.0".into()));
        assert_eq!(version_line("", "\ntesseract 4.0.0\n"), Some("tesseract 4.0.0".into()), "older versions print it on stderr");
        assert_eq!(version_line("  \n", ""), None);
    }

    #[test]
    fn list_langs_output_parses() {
        let out = "List of available languages in \"C:\\Program Files\\Tesseract-OCR\\tessdata/\" (2):\neng\nosd\n";
        assert_eq!(parse_list_langs(out), vec!["eng".to_string(), "osd".to_string()]);
        assert_eq!(parse_list_langs("List of available languages (1):\neng\n"), vec!["eng".to_string()]);
        assert!(parse_list_langs("").is_empty());
    }

    /// The real binary, when it is installed: a run end to end, with words that carry a
    /// confidence and the engine's name (skipped where `tesseract` is missing).
    #[test]
    fn recognize_runs_the_installed_binary() {
        let Some(t) = TesseractCli::find() else {
            eprintln!("skipped: the tesseract program is not installed");
            return;
        };
        assert_eq!(t.id(), "tesseract");
        assert!(t.available());
        // 300 dpi-ish block capitals, black on white.
        let (w, h, rgba) = hello_world_image();
        let image = OcrImage::new(w, h, rgba).unwrap();
        let lines = t.recognize(&image, &RecognizeOptions::default()).unwrap();
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert!(text.iter().any(|t| t.to_uppercase().contains("HELL")), "{text:?}");
        let words: Vec<&Word> = lines.iter().flat_map(|l| &l.words).collect();
        assert!(words.iter().all(|w| w.source == "tesseract"));
        assert!(words.iter().all(|w| matches!(w.confidence, Some(c) if (0.0..=100.0).contains(&c))), "{words:?}");
    }

    /// `available`/`languages` come from `--list-langs` (skipped without the binary).
    #[test]
    fn languages_come_from_list_langs() {
        let Some(t) = TesseractCli::find() else {
            eprintln!("skipped: the tesseract program is not installed");
            return;
        };
        let langs = t.languages();
        assert!(!langs.is_empty());
        assert!(langs.iter().all(|(c, n)| !c.is_empty() && !n.is_empty()), "{langs:?}");
    }

    /// A hung binary is killed at the timeout and reported (Unix: a fake `tesseract` script;
    /// this test needs a real process, so it is off the web and off Windows).
    #[cfg(unix)]
    #[test]
    fn a_hung_program_is_killed_at_the_timeout() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("tesseract-hang");
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        make_executable(&script);
        let mut t = TesseractCli::new(script);
        t.timeout = Duration::from_millis(500);
        let started = Instant::now();
        let r = t.recognize(&blot(8, 8, 0, 0, 4, 4), &RecognizeOptions::default());
        assert!(matches!(r, Err(OcrError::Timeout(_))), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(10), "killed, not waited out");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Output past the cap kills the run EARLY, while the child is still writing (Unix only,
    /// same fake-script trick): the script floods far past the cap and would sleep for 30 s
    /// after; a cap checked only after the exit would come back as a timeout instead.
    #[cfg(unix)]
    #[test]
    fn flooding_output_is_capped_and_killed_early() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-test-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("tesseract-flood");
        std::fs::write(&script, "#!/bin/sh\nhead -c 100000000 /dev/zero | tr '\\0' 'x'\nsleep 30\n").unwrap();
        make_executable(&script);
        let mut t = TesseractCli::new(script);
        t.max_output = 1024;
        t.timeout = Duration::from_secs(20);
        let started = Instant::now();
        let r = t.recognize(&blot(8, 8, 0, 0, 4, 4), &RecognizeOptions::default());
        assert!(matches!(r, Err(OcrError::OutputTooLarge(_))), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(10), "killed early, at {:?}", started.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An stderr flood does not block the child, so it does not cause a false timeout (Unix
    /// only): the script writes megabytes of stderr — far more than a pipe would swallow —
    /// then exits successfully with a small output, which must come back as a reading.
    #[cfg(unix)]
    #[test]
    fn an_stderr_flood_does_not_cause_a_false_timeout() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-test-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("tesseract-err-flood");
        std::fs::write(&script, "#!/bin/sh\nhead -c 20000000 /dev/zero | tr '\\0' 'e' >&2\necho flooded away\n").unwrap();
        make_executable(&script);
        let mut t = TesseractCli::new(script);
        t.timeout = Duration::from_secs(10);
        let r = t.recognize(&blot(8, 8, 0, 0, 4, 4), &RecognizeOptions::default());
        assert!(r.is_ok(), "stderr flood must not fail the run: {r:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The child runs with a minimal environment (its data prefix, the PATH, the Windows
    /// system root, and a temp dir), never the caller's whole environ. This is checked with a
    /// script that prints its own environ as the "TSV" (Unix only).
    #[cfg(unix)]
    #[test]
    fn the_child_gets_a_minimal_environment() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-test-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("tesseract-env");
        std::fs::write(&script, "#!/bin/sh\nenv | grep -v '^_='\n").unwrap();
        make_executable(&script);
        let t = TesseractCli::new(script);
        let png = encode_png(&blot(8, 8, 0, 0, 4, 4)).unwrap();
        std::fs::write(dir.join("page.png"), png).unwrap();
        let png_arg = dir.join("page.png").to_string_lossy().into_owned();
        let tsv = t.run(&[png_arg.as_str(), "stdout", "-l", "eng", "tsv"], t.timeout, t.max_output).unwrap();
        for (why, name) in [("the child's temp dir", "TMP="), ("temp", "TEMP="), ("lookup path", "PATH=")] {
            assert!(tsv.contains(name), "{why} must be passed: {tsv}");
        }
        // cargo always sets CARGO_* for the test process; env_clear must drop them.
        assert!(!tsv.contains("CARGO_"), "the caller's environment must not leak: {tsv}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    fn make_executable(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755));
    }

    /// The temp directory and its contents are gone after a run (private files do not linger).
    #[cfg(unix)]
    #[test]
    fn temp_files_are_deleted_and_private() {
        let dir = TempDir::create().unwrap();
        assert!(dir.path.is_dir());
        std::fs::write(dir.path.join("x"), b"secret").unwrap();
        let p = dir.path.clone();
        drop(dir);
        assert!(!p.exists(), "the temp directory is deleted after use");
    }

    /// A configured program location is only usable as an absolute path to an existing,
    /// executable file: a relative path, a missing file and a directory are unavailable (and
    /// [`TesseractCli::at`] refuses them without spawning anything).
    #[test]
    fn a_configured_program_must_be_an_absolute_executable_file() {
        assert!(!program_is_executable(Path::new(".")), "relative");
        assert!(!program_is_executable(Path::new("definitely/missing/tesseract")), "missing");
        let temp = std::env::temp_dir();
        assert!(!program_is_executable(&temp), "a directory is not a program");
        let file = temp.join(format!("pdfcraft-ocr-not-a-program-{}", std::process::id()));
        std::fs::write(&file, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644));
            assert!(!program_is_executable(&file), "a file without executable bits is not a program");
        }
        #[cfg(not(unix))]
        assert!(program_is_executable(&file), "an existing file is usable on this platform");
        let _ = std::fs::remove_file(&file);
        // `at` validates before any process starts, so these are None and not panics.
        assert!(TesseractCli::at(PathBuf::from(".")).is_none());
        assert!(TesseractCli::at(temp).is_none());
        assert!(TesseractCli::at(PathBuf::from("definitely/missing/tesseract")).is_none());
    }

    /// The PATH search resolves through absolute entries only and skips relative ones: a
    /// relative PATH entry would resolve against the caller's working directory.
    #[test]
    fn path_search_skips_relative_entries() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join(program_file_names()[0]);
        std::fs::write(&program, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755));
        }
        let found = path_search_in([PathBuf::from("relative/dir"), PathBuf::from("definitely/missing"), dir.clone()].into_iter());
        assert_eq!(found.as_deref(), Some(program.as_path()), "the absolute entry with the program wins, relative entries are skipped");
        let none = path_search_in([PathBuf::from("relative/dir")].into_iter());
        assert_eq!(none, None, "only relative entries: nothing is found");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// "HELLO" as blocky 5×7 glyphs, 8 pixels per dot with wide tracking, at roughly 300 dpi
    /// scale (each dot ~10 px): big enough for Tesseract's word detector.
    fn hello_world_image() -> (u32, u32, Vec<u8>) {
        const GLYPHS: &[&str] = &[
            "X...X", "X...X", "X...X", "XXXXX", "X...X", "X...X", "X...X", // H
        ];
        // Draw the five glyphs of HELLO with per-letter shapes for E, L, L, O.
        const E: &[&str] = &["XXXXX", "X....", "X....", "XXXX.", "X....", "X....", "XXXXX"];
        const L: &[&str] = &["X....", "X....", "X....", "X....", "X....", "X....", "XXXXX"];
        const O: &[&str] = &[".XXX.", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX."];
        let letters: [&[&str]; 5] = [GLYPHS, E, L, L, O];
        let (dot, x0, y0, advance) = (8u32, 40u32, 40u32, 7u32 * 8);
        let w = x0 * 2 + advance * 5;
        let h = y0 * 2 + 7 * dot;
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        for (i, rows) in letters.iter().enumerate() {
            for (ry, row) in rows.iter().enumerate() {
                for (rx, b) in row.bytes().enumerate() {
                    if b != b'X' {
                        continue;
                    }
                    for dy in 0..dot {
                        for dx in 0..dot {
                            let x = x0 + i as u32 * advance + rx as u32 * dot + dx;
                            let y = y0 + ry as u32 * dot + dy;
                            let o = ((y * w + x) * 4) as usize;
                            rgba[o..o + 3].copy_from_slice(&[0, 0, 0]);
                        }
                    }
                }
            }
        }
        (w, h, rgba)
    }
}

#[cfg(all(test, unix))]
mod process_tests {
    use super::*;

    /// A stderr flood is killed at the cap while the child still runs, not accepted until it
    /// exits (an endless writer would otherwise run out the whole timeout and fill the disk).
    /// The fixture needs a Unix shell, like the other process fixtures.
    #[test]
    fn a_stderr_flood_is_killed_while_the_child_runs() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-tesseract-flood-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("flood-tesseract");
        std::fs::write(&program, "#!/bin/sh\nwhile true; do echo aaaaaaaaaaaaaaaaaaaaaaaa >&2; done\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&program).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&program, perms).unwrap();
        let t = TesseractCli::new(program);
        let started = std::time::Instant::now();
        let err = t.run_capture(&["--list-langs"], Duration::from_secs(60), 1024 * 1024).unwrap_err();
        assert!(matches!(err, OcrError::OutputTooLarge(_)), "{err}");
        assert!(started.elapsed() < Duration::from_secs(30), "killed at the cap, not at the timeout");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
