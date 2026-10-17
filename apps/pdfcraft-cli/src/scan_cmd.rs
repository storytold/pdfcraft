//! `pdfcraft-cli scan` as one `ScanJob`: the main thread owns the job and polls while a worker thread
//! runs `acquire`, pages are written to the output folder as they arrive, and the journal
//! (`scan-job.json`) is rewritten at every state change. Also `scan jobs`, `scan job DIR` and
//! `scan --continue DIR`. See docs/scan-stack.md.
//!
//! No Ctrl-C handler: std has none, and a safe one needs a dependency or `unsafe` (this crate forbids
//! the latter). Terminating the process leaves the journal as it was at the last change (state
//! `acquiring`), which every reader treats as an unknown outcome.

use std::path::{Path, PathBuf};
use std::time::Duration;

use craft_job::{RetryAdvice, Timestamp};
use pdfcraft_automation::scan_jobs::{self, JobRecord};
use pdfcraft_platform::scan::{
    self, Page, ScanError, ScanSettings,
    job::{Continuation, ExitClass, Outcome, ScanJob, ScanJobConfig, ScanJobError, ScanJobState},
};
use serde_json::{Value, json};

use crate::{CliError, stdout_line};

/// How a run starts.
pub struct Run {
    pub folder: PathBuf,
    pub settings: ScanSettings,
    pub reverse: bool,
    /// Fronts of a manual-duplex pass, already in the folder.
    pub front_files: Option<Vec<PathBuf>>,
    /// Set by `--continue`: the journal that is being continued and what to do with its pages.
    pub resume: Option<(JobRecord, Continuation)>,
}

fn now() -> Timestamp {
    Timestamp::from_millis(scan_jobs::now_millis())
}

/// The journal writer: everything needed to rewrite `scan-job.json` from the job's current state.
struct Journal<'a> {
    folder: &'a Path,
    id: String,
    started: u64,
    request: Value,
    prior_attempts: Vec<Value>,
}

impl Journal<'_> {
    fn save(&self, job: &ScanJob) -> Result<(), CliError> {
        let snapshot = job.snapshot();
        let mut attempts = self.prior_attempts.clone();
        attempts.extend(snapshot.attempts.iter().filter_map(|attempt| serde_json::to_value(attempt).ok()));
        let record = JobRecord {
            id: self.id.clone(),
            started: self.started,
            updated: scan_jobs::now_millis(),
            request: self.request.clone(),
            journal: snapshot.into_journal(),
            view: serde_json::to_value(job.view()).unwrap_or(Value::Null),
            attempts,
        };
        scan_jobs::write_record(self.folder, &record).map_err(|error| CliError::from(format!("scan: cannot write the job file: {error}")))
    }
}

/// Write pages `*written..` of the job's held pages into the folder, never over an existing file.
fn flush_pages(job: &ScanJob, folder: &Path, base: usize, written: &mut usize, saved: &mut Vec<Value>) -> Result<(), String> {
    use std::io::Write;
    for (index, page) in job.pages().iter().enumerate().skip(*written) {
        let path = scan::page_path(folder, base + index + 1, page.format);
        let mut file =
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        file.write_all(&page.bytes).map_err(|error| format!("{}: {error}", path.display()))?;
        eprintln!("pdfcraft-cli: saved {}", path.display());
        saved.push(crate::page_json(&path, page));
        *written = index + 1;
    }
    Ok(())
}

fn job_error(error: &ScanJobError) -> CliError {
    CliError::from(format!("scan: {error}"))
}

/// Run the job to its end (or to the point where a person must act) and exit by its class.
pub fn run(spec: Run) -> Result<(), CliError> {
    crate::ensure_scanning_supported()?;
    let Run { folder, settings, reverse, front_files, resume } = spec;
    let started_at = scan_jobs::now_millis();
    let title = folder.file_name().and_then(|name| name.to_str()).filter(|name| !name.is_empty()).unwrap_or("Scan").to_string();
    let config = ScanJobConfig::default().with_title(title).with_created(now());
    let (id, started, prior_attempts, how, mut job) = match resume {
        Some((record, how)) => {
            let mut journal = record.journal.clone();
            // A file left in `acquiring` by a process that ended mid-scan is an unknown outcome.
            journal.state = record.effective_state();
            let job = ScanJob::continuing_from(settings.clone(), config, journal);
            (record.id.clone(), record.started, record.attempts.clone(), Some(how), job)
        }
        None => (format!("scan-{started_at}-{:x}", std::process::id()), started_at, Vec::new(), None, ScanJob::new(settings.clone(), config)),
    };
    let journal = Journal { folder: &folder, id, started, request: scan_jobs::request_json(&settings, reverse), prior_attempts };

    let mut saved: Vec<Value> = Vec::new();
    let mut written = 0usize;
    let mut first_base: Option<usize> = None;
    let mut first = true;
    let mut write_error: Option<String> = None;
    loop {
        let plan = if let (true, Some(how)) = (first, how) { job.begin_attempt_with(now(), how) } else { job.begin_attempt(now()) }
            .map_err(|error| job_error(&error))?;
        first = false;
        let base = *first_base.get_or_insert(usize::try_from(plan.kept_pages).unwrap_or(0));
        journal.save(&job)?;
        let worker_settings = plan.settings.clone();
        let worker = std::thread::spawn(move || scan::acquire(&worker_settings));
        while !worker.is_finished() {
            let got = job.record_progress(&plan.progress);
            if got > 0 {
                if !reverse
                    && write_error.is_none()
                    && let Err(error) = flush_pages(&job, &folder, base, &mut written, &mut saved)
                {
                    // Out of space or a file in the way: stop the transfer rather than pull more paper.
                    write_error = Some(error);
                    job.cancel(now());
                }
                journal.save(&job)?;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let result = worker.join().unwrap_or_else(|_| Err(ScanError::Failed("the scan worker stopped unexpectedly".into())));
        job.finish_attempt(result, now()).map_err(|error| job_error(&error))?;
        if !reverse
            && write_error.is_none()
            && let Err(error) = flush_pages(&job, &folder, base, &mut written, &mut saved)
        {
            write_error = Some(error);
        }
        journal.save(&job)?;
        match job.retry_advice() {
            RetryAdvice::RetryNow if write_error.is_none() && !job.state().is_terminal() => {
                eprintln!("pdfcraft-cli: trying again ({})", job.snapshot().summary_line())
            }
            RetryAdvice::RetryAfter(wait) if write_error.is_none() && !job.state().is_terminal() => {
                eprintln!("pdfcraft-cli: scanner busy, trying again in {} s", wait.as_secs().max(1));
                std::thread::sleep(wait);
            }
            _ => break,
        }
    }

    let snapshot = job.snapshot();
    let state = job.state();
    let class = state.exit_class();
    let mut reverse_note = None;
    if reverse {
        if state == ScanJobState::Terminal(Outcome::Completed) {
            let backs: Vec<Page> = job.take_pages().map_err(|error| job_error(&error))?;
            match crate::save_reverse_pass(&folder, front_files.as_deref().unwrap_or(&[]), &backs) {
                Ok(pages) => saved = pages,
                Err(error) => reverse_note = Some(error.to_string()),
            }
        } else {
            reverse_note = Some("the reverse pass did not finish: the fronts in the folder are unchanged and the backs scanned so far were not kept; run --reverse again".to_string());
        }
    }
    let class = if write_error.is_some() || reverse_note.is_some() { ExitClass::Failed } else { class };
    let code = scan_jobs::exit_code(class);
    let source = snapshot.source_in_use.map_or(settings.source.name(), |source| source.name());
    let result = json!({
        "source": source,
        "requested_source": settings.source.name(),
        "reverse": reverse,
        "count": saved.len(),
        "pages": saved,
        "state": state.name(),
        "exit_class": class.name(),
        "exit_code": code,
        "journal": folder.join(scan_jobs::JOB_FILE).display().to_string(),
        "job": serde_json::to_value(job.view()).unwrap_or(Value::Null),
    });
    stdout_line(format_args!("{}", serde_json::to_string(&result).map_err(|error| format!("scan: {error}"))?))?;
    if code == 0 {
        return Ok(());
    }
    let (message, json) = match (&write_error, &reverse_note, &snapshot.last_error) {
        (Some(error), _, _) => (format!("scan: could not save a page: {error}"), None),
        (None, Some(note), _) => (format!("scan: {note}"), None),
        (None, None, Some(failure)) => {
            let info = error_line(&failure.info);
            (format!("scan: {}", failure.info.message), Some(info))
        }
        (None, None, None) => (format!("scan: {}", snapshot.summary_line()), None),
    };
    let hint = match class {
        ExitClass::NeedsAttention | ExitClass::Failed => {
            format!("\n  job: {}\n  see: pdfcraft-cli scan job {}", snapshot.summary_line(), folder.display())
        }
        _ => String::new(),
    };
    Err(CliError::Exit { code, message: format!("{message}{hint}"), json })
}

/// The failure's identity as one line of JSON for stderr: `{"scan_error": {kind, phase, hresult, code_name,
/// retryable, resumable, message}}`, so a caller reads fields instead of the sentence.
fn error_line(info: &scan::ScanErrorInfo) -> String {
    json!({ "scan_error": pdfcraft_automation::scan_error_info_json(info) }).to_string()
}

/// What `--continue` decides, from the journal and the folder alone (so it can be tested without a scanner).
#[derive(Debug, PartialEq, Eq)]
pub struct ContinueFlags {
    pub append: bool,
    pub restart: bool,
    pub checked: bool,
}

/// Apply the continuation rules. `Ok(how)` is what to do with the kept pages; an `Err` is the sentence to
/// show, and nothing has been touched.
pub fn continuation_for(record: &JobRecord, files_on_disk: Result<usize, String>, flags: &ContinueFlags) -> Result<Continuation, String> {
    if flags.append && flags.restart {
        return Err("scan --continue: give --append or --restart, not both".to_string());
    }
    let state = record.effective_state();
    if state == ScanJobState::Terminal(Outcome::Completed) {
        return Err("scan --continue: this job is complete; there is nothing to continue (scan into a new folder)".to_string());
    }
    if record.is_reverse() {
        return Err("scan --continue: a manual-duplex --reverse pass cannot be continued; run --reverse again (the fronts are untouched)".to_string());
    }
    let kept = record.pages_kept();
    let on_disk = files_on_disk.map_err(|reason| format!("scan --continue: {reason}"))?;
    if on_disk != kept {
        return Err(format!(
            "scan --continue: the journal keeps {kept} page(s) but {on_disk} page file(s) are in the folder; the journal does not verify files, so fix the folder (or start over in a new one) before continuing"
        ));
    }
    if state == ScanJobState::NeedsAttention && !flags.checked {
        let why = if record.was_cut_off() {
            "the process that ran this job ended without finishing it"
        } else {
            "what the scanner did after the last attempt is not known (the link dropped, or an error came after pages or part-way through one)"
        };
        return Err(format!(
            "scan --continue: this job needs attention: {why}. Look at the scanner first: a repeated acquire moves the carriage and pulls paper through the feeder, and sheets that were already scanned are ejected and are not scanned again unless you put them back. When you have checked, add --i-checked-the-scanner"
        ));
    }
    if kept == 0 {
        // Nothing is kept, so append and restart are the same.
        return Ok(Continuation::Restart);
    }
    match (flags.append, flags.restart) {
        (true, _) => {
            if let Some(wanted) = record.journal.expected_pages
                && u32::try_from(kept).unwrap_or(u32::MAX) >= wanted
            {
                return Err(format!(
                    "scan --continue: all {wanted} wanted page(s) are already kept; nothing to append (--restart scans everything again)"
                ));
            }
            Ok(Continuation::Append)
        }
        (_, true) => Ok(Continuation::Restart),
        _ => Err(format!(
            "scan --continue: {kept} page(s) are kept, so say what to do with them:\n  --append   keep them and scan only the missing ones (put the remaining sheets in the feeder)\n  --restart  scan everything again; the kept page files are moved aside into discarded-N, not deleted"
        )),
    }
}

/// Move the kept page files (and a copy of the journal) into `DIR/discarded-<n>`; never deletes anything.
pub fn move_aside(folder: &Path) -> Result<PathBuf, String> {
    let files = scan::listed_page_files(folder).map_err(|error| error.to_string())?;
    let mut n = 1;
    let aside = loop {
        let candidate = folder.join(format!("discarded-{n}"));
        if !candidate.exists() {
            break candidate;
        }
        n += 1;
    };
    std::fs::create_dir_all(&aside).map_err(|error| format!("{}: {error}", aside.display()))?;
    let journal = folder.join(scan_jobs::JOB_FILE);
    if journal.is_file() {
        std::fs::copy(&journal, aside.join(scan_jobs::JOB_FILE)).map_err(|error| format!("{}: {error}", journal.display()))?;
    }
    for file in files {
        let name = file.file_name().ok_or("a page file has no name")?;
        std::fs::rename(&file, aside.join(name)).map_err(|error| format!("{}: {error}", file.display()))?;
    }
    Ok(aside)
}

/// `scan --continue DIR`: read the journal and request, apply the rules, then run the same job loop.
pub fn continue_job(folder: &Path, flags: &ContinueFlags) -> Result<(), CliError> {
    let record = scan_jobs::read_record(folder).map_err(|error| format!("scan --continue: {error}"))?;
    let on_disk = scan_jobs::page_files_on_disk(folder);
    let how = continuation_for(&record, on_disk, flags)?;
    let settings = scan_jobs::settings_from_request(&record.request).map_err(|error| format!("scan --continue: {error}"))?;
    settings.checked().map_err(|error| format!("scan --continue: {error}"))?;
    crate::ensure_scanning_supported()?;
    if how == Continuation::Restart && record.pages_kept() > 0 {
        let aside = move_aside(folder).map_err(|error| format!("scan --continue: {error}"))?;
        eprintln!("pdfcraft-cli: moved the kept pages aside into {}", aside.display());
    }
    run(Run { folder: folder.to_path_buf(), settings, reverse: false, front_files: None, resume: Some((record, how)) })
}

/// `scan jobs [DIR...] [--json]`: list the journals. Opens nothing but files.
pub fn jobs(args: &[String]) -> Result<(), CliError> {
    let mut json = false;
    let mut dirs = Vec::new();
    for arg in &args[1..] {
        match arg.as_str() {
            "--json" => json = true,
            other if other.starts_with("--") => return Err(format!("scan jobs takes only DIR... and --json (got {other})").into()),
            dir => dirs.push(PathBuf::from(dir)),
        }
    }
    let current = std::env::current_dir().map_err(|error| format!("scan jobs: {error}"))?;
    let (lines, rows) = scan_jobs::jobs_listing(&dirs, &current);
    if json {
        return stdout_line(format_args!("{}", serde_json::to_string(&rows).map_err(|error| format!("scan jobs: {error}"))?));
    }
    if lines.is_empty() {
        return stdout_line(format_args!("no scan jobs found"));
    }
    for line in lines {
        stdout_line(format_args!("{line}"))?;
    }
    Ok(())
}

/// `scan job DIR [--json]`: one job in detail. Opens nothing but files.
pub fn job(args: &[String]) -> Result<(), CliError> {
    let mut json = false;
    let mut dirs = Vec::new();
    for arg in &args[1..] {
        match arg.as_str() {
            "--json" => json = true,
            other if other.starts_with("--") => return Err(format!("scan job takes only DIR and --json (got {other})").into()),
            dir => dirs.push(PathBuf::from(dir)),
        }
    }
    let [dir] = dirs.as_slice() else {
        return Err("scan job needs exactly one DIR (scan jobs lists them)".into());
    };
    let record = scan_jobs::read_record(dir).map_err(|error| format!("scan job: {error}"))?;
    if json {
        return stdout_line(format_args!(
            "{}",
            serde_json::to_string(&scan_jobs::job_json(dir, &record)).map_err(|error| format!("scan job: {error}"))?
        ));
    }
    stdout_line(format_args!("{}", scan_jobs::detail_text(scan_jobs::PROGRAM, dir, &record).trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_platform::scan::job::ScanJobJournal;

    fn record(state: ScanJobState, kept: usize, expected: Option<u32>) -> JobRecord {
        let settings = ScanSettings { pages: expected.unwrap_or(0), ..ScanSettings::default() };
        let mut journal: ScanJobJournal = ScanJob::new(settings.clone(), ScanJobConfig::default()).snapshot().into_journal();
        journal.state = state;
        let page = serde_json::json!({ "number": 1, "attempt": 1, "format": "bmp", "width": 2, "height": 2, "dpi": 200, "bytes": 8 });
        journal.pages = (0..kept).map(|_| serde_json::from_value(page.clone()).unwrap()).collect();
        JobRecord {
            id: "scan-1".into(),
            started: 1,
            updated: 2,
            request: scan_jobs::request_json(&settings, false),
            journal,
            view: Value::Null,
            attempts: Vec::new(),
        }
    }

    #[test]
    fn a_failed_scan_adds_its_identity_as_one_json_line() {
        use pdfcraft_platform::scan::{InterruptKind, NativeCode};
        let settings = ScanSettings { pages: 2, ..ScanSettings::default() };
        let error = ScanError::native_error(
            InterruptKind::MultiFeed,
            NativeCode::hresult(0x8021_0020, Some("WIA_ERROR_MULTI_FEED")),
            "The feeder took more than one sheet.",
        );
        let info = scan::ScanErrorInfo::from_acquire(&error, &settings);
        let value: Value = serde_json::from_str(&error_line(&info)).unwrap();
        let info = &value["scan_error"];
        assert_eq!(info["kind"], "multi_feed");
        assert_eq!(info["phase"], "before_first_page");
        assert_eq!(info["hresult"], "0x80210020");
        assert_eq!(info["code_name"], "WIA_ERROR_MULTI_FEED");
        assert_eq!((info["retryable"].clone(), info["resumable"].clone()), (json!(true), json!(false)));
    }

    const NONE: ContinueFlags = ContinueFlags { append: false, restart: false, checked: false };

    #[test]
    fn continuing_with_pages_kept_needs_an_explicit_choice() {
        let record = record(ScanJobState::Interrupted, 2, Some(5));
        let message = continuation_for(&record, Ok(2), &NONE).unwrap_err();
        assert!(message.contains("--append") && message.contains("--restart"), "{message}");
        assert_eq!(continuation_for(&record, Ok(2), &ContinueFlags { append: true, ..NONE }), Ok(Continuation::Append));
        assert_eq!(continuation_for(&record, Ok(2), &ContinueFlags { restart: true, ..NONE }), Ok(Continuation::Restart));
        let both = ContinueFlags { append: true, restart: true, checked: false };
        assert!(continuation_for(&record, Ok(2), &both).unwrap_err().contains("not both"));
    }

    #[test]
    fn continuing_with_nothing_kept_needs_no_choice() {
        let record = record(ScanJobState::Terminal(Outcome::Failed), 0, Some(5));
        assert_eq!(continuation_for(&record, Ok(0), &NONE), Ok(Continuation::Restart));
    }

    #[test]
    fn needs_attention_is_refused_without_the_acknowledgement() {
        let record = record(ScanJobState::NeedsAttention, 2, Some(5));
        let flags = ContinueFlags { append: true, ..NONE };
        let message = continuation_for(&record, Ok(2), &flags).unwrap_err();
        assert!(message.contains("--i-checked-the-scanner") && message.contains("carriage"), "{message}");
        let checked = ContinueFlags { append: true, checked: true, restart: false };
        assert_eq!(continuation_for(&record, Ok(2), &checked), Ok(Continuation::Append));
    }

    #[test]
    fn a_job_cut_off_while_acquiring_counts_as_needs_attention() {
        let record = record(ScanJobState::Acquiring, 0, Some(5));
        let message = continuation_for(&record, Ok(0), &NONE).unwrap_err();
        assert!(message.contains("ended without finishing"), "{message}");
        let checked = ContinueFlags { checked: true, ..NONE };
        assert_eq!(continuation_for(&record, Ok(0), &checked), Ok(Continuation::Restart));
    }

    #[test]
    fn a_complete_job_a_reverse_pass_and_a_changed_folder_are_refused() {
        let done = record(ScanJobState::Terminal(Outcome::Completed), 5, Some(5));
        assert!(continuation_for(&done, Ok(5), &NONE).unwrap_err().contains("complete"));
        let mut reverse = record(ScanJobState::Interrupted, 1, Some(2));
        reverse.request["reverse"] = json!(true);
        assert!(continuation_for(&reverse, Ok(1), &NONE).unwrap_err().contains("--reverse"));
        let interrupted = record(ScanJobState::Interrupted, 2, Some(5));
        let flags = ContinueFlags { append: true, ..NONE };
        assert!(continuation_for(&interrupted, Ok(3), &flags).unwrap_err().contains("3 page file(s)"));
        assert!(continuation_for(&interrupted, Err("gap".into()), &flags).unwrap_err().contains("gap"));
    }

    #[test]
    fn append_with_everything_kept_has_nothing_to_scan() {
        let record = record(ScanJobState::Terminal(Outcome::CompletedWithWarnings), 5, Some(5));
        let flags = ContinueFlags { append: true, ..NONE };
        assert!(continuation_for(&record, Ok(5), &flags).unwrap_err().contains("nothing to append"));
        assert_eq!(continuation_for(&record, Ok(5), &ContinueFlags { restart: true, ..NONE }), Ok(Continuation::Restart));
    }

    #[test]
    fn restart_moves_pages_aside_and_never_deletes() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-scan-aside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("discarded-1")).unwrap();
        std::fs::write(dir.join("page-001.bmp"), b"one").unwrap();
        std::fs::write(dir.join("page-002.bmp"), b"two").unwrap();
        std::fs::write(dir.join("scan-job.json"), b"{}").unwrap();
        std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
        let aside = move_aside(&dir).unwrap();
        assert_eq!(aside, dir.join("discarded-2"), "an existing discarded-N is not reused");
        assert_eq!(std::fs::read(aside.join("page-002.bmp")).unwrap(), b"two");
        assert!(aside.join("scan-job.json").is_file());
        assert!(!dir.join("page-001.bmp").exists());
        assert!(dir.join("notes.txt").is_file(), "files that are not pages stay");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- the whole loop against a scripted in-process backend (no scanner, nothing opened) -----------------

    use std::collections::VecDeque;
    use std::sync::{Mutex, MutexGuard, Once};

    use pdfcraft_platform::scan::{Acquired, Backend, Cause, HandlingStatus, InterruptKind, Scanner, ScannerRef, Source};

    /// What the next `acquire` of the mock does.
    enum Step {
        /// All of the pages asked for arrive.
        Complete,
        /// This many pages arrive, then the feeder jams.
        JamAfter(usize),
        /// The link drops with no page in hand.
        Offline,
    }

    static SCRIPT: Mutex<VecDeque<Step>> = Mutex::new(VecDeque::new());
    static ASKED: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    static SERIAL: Mutex<()> = Mutex::new(());

    struct Mock;

    fn page(n: u8) -> Page {
        Page {
            bytes: vec![n; 8],
            format: "bmp",
            width: 2,
            height: 2,
            dpi: 200,
            dpi_y: 200,
            requested_dpi: 200,
            thresholded: false,
            length: Default::default(),
        }
    }

    impl Backend for Mock {
        fn name(&self) -> &'static str {
            "mock"
        }
        fn scanner_list(&self) -> Result<Vec<ScannerRef>, ScanError> {
            Ok(Vec::new())
        }
        fn scanner_details(&self, _: &str) -> Result<Scanner, ScanError> {
            Err(ScanError::Failed("the mock has no details".into()))
        }
        fn scanners(&self) -> Result<Vec<Scanner>, ScanError> {
            Ok(Vec::new())
        }
        fn status(&self, _: &str) -> Option<HandlingStatus> {
            None
        }
        fn diagnostic_dump(&self, _: &str) -> Result<String, ScanError> {
            Ok(String::new())
        }
        fn acquire(&self, settings: &ScanSettings) -> Result<Acquired, ScanError> {
            ASKED.lock().unwrap().push(settings.pages);
            let progress = settings.progress.clone().expect("the job gives every attempt a progress token");
            let step = SCRIPT.lock().unwrap().pop_front().expect("a scripted step");
            let wanted = if settings.pages == 0 { 1 } else { settings.pages as usize };
            match step {
                Step::Complete => {
                    let pages: Vec<Page> = (0..wanted).map(|n| page(n as u8)).collect();
                    for page in &pages {
                        progress.push_page(page.clone());
                    }
                    Ok(Acquired { source: Source::Feeder, pages })
                }
                Step::JamAfter(n) => {
                    let pages: Vec<Page> = (0..n).map(|k| page(k as u8)).collect();
                    for page in &pages {
                        progress.push_page(page.clone());
                    }
                    Err(ScanError::Incomplete {
                        message: "The feeder jammed.".into(),
                        acquired: Acquired { source: Source::Feeder, pages },
                        cause: Some(Cause { kind: InterruptKind::Jam, native: None }),
                    })
                }
                Step::Offline => Err(ScanError::Native { kind: InterruptKind::Offline, native: None, message: "The scanner went offline.".into() }),
            }
        }
    }

    fn mock_lock() -> MutexGuard<'static, ()> {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let _ = scan::register(Box::new(Mock));
        });
        let guard = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        SCRIPT.lock().unwrap().clear();
        ASKED.lock().unwrap().clear();
        guard
    }

    fn script(steps: Vec<Step>) {
        *SCRIPT.lock().unwrap() = steps.into();
    }

    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-scan-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn start(folder: &Path, pages: u32) -> Result<(), CliError> {
        let settings = ScanSettings { device: "mock:1".into(), source: scan::Source::Feeder, pages, ..ScanSettings::default() };
        run(Run { folder: folder.to_path_buf(), settings, reverse: false, front_files: None, resume: None })
    }

    fn exit_code_of(result: Result<(), CliError>) -> u8 {
        match result {
            Ok(()) => 0,
            Err(CliError::Exit { code, .. }) => code,
            Err(other) => panic!("not a job exit: {other}"),
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    fn refuse(dir: &Path) -> String {
        crate::refuse_a_folder_that_has_a_job(dir, false).unwrap_err().to_string()
    }

    #[test]
    fn a_scan_that_completes_writes_pages_and_a_completed_journal() {
        let _serial = mock_lock();
        let dir = fresh("complete");
        script(vec![Step::Complete]);
        assert_eq!(exit_code_of(start(&dir, 3)), 0);
        assert_eq!(names(&dir), ["page-001.bmp", "page-002.bmp", "page-003.bmp", "scan-job.json"]);
        let record = scan_jobs::read_record(&dir).unwrap();
        assert_eq!(record.journal.state, ScanJobState::Terminal(Outcome::Completed));
        assert_eq!((record.pages_kept(), record.journal.expected_pages, record.journal.attempts_total), (3, Some(3), 1));
        assert_eq!(record.view["state"], "completed");
        assert_eq!(scan_jobs::settings_from_request(&record.request).unwrap().device, "mock:1");
        assert!(scan_jobs::page_mismatch(&dir, &record).is_none());
        // The journal describes pages and holds none of their bytes.
        let text = std::fs::read_to_string(dir.join("scan-job.json")).unwrap();
        assert!(!text.contains("\"data\"") && text.len() < 20_000, "{}", text.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_jam_keeps_the_pages_and_continue_append_scans_only_the_rest() {
        let _serial = mock_lock();
        let dir = fresh("jam");
        script(vec![Step::JamAfter(1)]);
        assert_eq!(exit_code_of(start(&dir, 3)), 10, "an interrupted job needs attention");
        assert_eq!(names(&dir), ["page-001.bmp", "scan-job.json"]);
        let record = scan_jobs::read_record(&dir).unwrap();
        assert_eq!(record.journal.state, ScanJobState::Interrupted);
        assert_eq!(record.last_error_kind(), "jam");
        let line = scan_jobs::list_line(&dir, &record);
        assert!(line.contains("  interrupted  1/3  1 attempt(s)  jam  "), "{line}");
        // A second fresh scan into the same folder is refused.
        assert!(refuse(&dir).contains("already holds a scan job"));
        // Continue without a choice: refused, nothing scanned.
        let none = continue_job(&dir, &NONE).unwrap_err().to_string();
        assert!(none.contains("--append") && none.contains("--restart"), "{none}");
        assert_eq!(ASKED.lock().unwrap().len(), 1, "nothing was scanned by the refusal");
        // Append: only the two missing pages are asked for, and they land after page 1.
        script(vec![Step::Complete]);
        assert_eq!(exit_code_of(continue_job(&dir, &ContinueFlags { append: true, ..NONE })), 0);
        assert_eq!(*ASKED.lock().unwrap(), [3, 2]);
        assert_eq!(names(&dir), ["page-001.bmp", "page-002.bmp", "page-003.bmp", "scan-job.json"]);
        let record = scan_jobs::read_record(&dir).unwrap();
        assert_eq!(record.journal.state, ScanJobState::Terminal(Outcome::Completed));
        assert_eq!((record.pages_kept(), record.journal.attempts_total), (3, 2));
        assert_eq!(record.attempts.len(), 2, "the timeline of both runs is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restart_moves_the_kept_pages_aside_and_scans_everything_again() {
        let _serial = mock_lock();
        let dir = fresh("restart");
        script(vec![Step::JamAfter(2)]);
        assert_eq!(exit_code_of(start(&dir, 3)), 10);
        script(vec![Step::Complete]);
        assert_eq!(exit_code_of(continue_job(&dir, &ContinueFlags { restart: true, ..NONE })), 0);
        assert_eq!(*ASKED.lock().unwrap(), [3, 3]);
        assert_eq!(names(&dir), ["discarded-1", "page-001.bmp", "page-002.bmp", "page-003.bmp", "scan-job.json"]);
        assert_eq!(names(&dir.join("discarded-1")), ["page-001.bmp", "page-002.bmp", "scan-job.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_outcome_waits_for_the_acknowledgement() {
        let _serial = mock_lock();
        let dir = fresh("offline");
        script(vec![Step::Offline]);
        assert_eq!(exit_code_of(start(&dir, 2)), 10);
        let record = scan_jobs::read_record(&dir).unwrap();
        assert_eq!(record.journal.state, ScanJobState::NeedsAttention);
        let refused = continue_job(&dir, &ContinueFlags { append: true, ..NONE }).unwrap_err().to_string();
        assert!(refused.contains("--i-checked-the-scanner"), "{refused}");
        assert_eq!(ASKED.lock().unwrap().len(), 1);
        script(vec![Step::Complete]);
        let checked = ContinueFlags { append: true, checked: true, restart: false };
        assert_eq!(exit_code_of(continue_job(&dir, &checked)), 0);
        assert_eq!(names(&dir), ["page-001.bmp", "page-002.bmp", "scan-job.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_job_file_left_acquiring_is_read_as_unknown() {
        let _serial = mock_lock();
        let dir = fresh("killed");
        script(vec![Step::Complete]);
        assert_eq!(exit_code_of(start(&dir, 1)), 0);
        let mut record = scan_jobs::read_record(&dir).unwrap();
        record.journal.state = ScanJobState::Acquiring;
        scan_jobs::write_record(&dir, &record).unwrap();
        let refused = continue_job(&dir, &NONE).unwrap_err().to_string();
        assert!(refused.contains("ended without finishing"), "{refused}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
