//! `pdfcraft-cli scan` as one `ScanJob`: the main thread owns the job and polls while a worker thread
//! runs `acquire`, pages are written to the output folder as they arrive, and the journal
//! (`scan-job.json`) is rewritten at every state change. Also `scan jobs`, `scan job DIR` and
//! `scan --continue DIR`. See docs/scan-stack.md.
//!
//! A pile larger than the feeder is one job scanned in batches: `--continue --append` adds the next feeder
//! load of fronts. Manual duplex is the same job going the other way: `--continue --reverse` turns it (the
//! whole stack flipped, the last sheet first) and every later `--continue --append` scans the next part of
//! the flipped stack, the backs of the sheets below the cursor. Every page is written to its place in the
//! document as it arrives (see `craft_devices::scan_jobs::duplex_page_number`), so nothing is held back
//! and nothing is rearranged at the end.
//!
//! No Ctrl-C handler: std has none, and a safe one needs a dependency or `unsafe` (this crate forbids
//! the latter). Terminating the process leaves the journal as it was at the last change (state
//! `acquiring`), which every reader treats as an unknown outcome.

use std::path::{Path, PathBuf};
use std::time::Duration;

use craft_job::{RetryAdvice, Timestamp};
use pdfcraft_automation::scan_jobs::{self, JobRecord, ManualDuplex};
use pdfcraft_platform::scan::{
    self, ScanError, ScanSettings,
    job::{Continuation, ExitClass, Outcome, ScanJob, ScanJobConfig, ScanJobError, ScanJobState},
};
use serde_json::{Value, json};

use crate::{CliError, stdout_line};

/// How a run starts.
pub struct Run {
    pub folder: PathBuf,
    pub settings: ScanSettings,
    /// Set by `--continue`: the journal that is being continued and what to do with its pages.
    pub resume: Option<(JobRecord, Continuation)>,
    /// Set once the job has turned for its backs: where the pages go and how far down the stack it is.
    pub duplex: Option<ManualDuplex>,
}

/// The current wall-clock time as a job timestamp.
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
    duplex: Option<ManualDuplex>,
}

impl Journal<'_> {
    /// Rewrite the job file from the job's current snapshot, adding this run's attempts after the earlier runs'. The
    /// pages are renumbered by the file each one is saved in.
    fn save(&self, job: &ScanJob) -> Result<(), CliError> {
        let snapshot = job.snapshot();
        let mut attempts = self.prior_attempts.clone();
        attempts.extend(snapshot.attempts.iter().filter_map(|attempt| serde_json::to_value(attempt).ok()));
        let mut record = JobRecord {
            id: self.id.clone(),
            started: self.started,
            updated: scan_jobs::now_millis(),
            request: self.request.clone(),
            journal: snapshot.into_journal(),
            view: serde_json::to_value(job.view()).unwrap_or(Value::Null),
            attempts,
            manual_duplex: self.duplex.map(|duplex| duplex.after(job.page_count())),
        };
        // Each page record names the file it is (a manual duplex writes them out of scan order).
        record.number_pages_by_file();
        scan_jobs::write_record(self.folder, &record).map_err(|error| CliError::from(format!("scan: cannot write the job file: {error}")))
    }
}

/// Where the job's page `index` (0-based, scan order) is written: `page-(index+1)` for a single-sided job,
/// its place in the document for a manual-duplex one.
fn page_number(duplex: Option<ManualDuplex>, index: usize) -> usize {
    match duplex {
        Some(duplex) => scan_jobs::duplex_page_number(duplex.sheets, index),
        None => index + 1,
    }
}

/// Create `path` (never over an existing file) and fill it with `write`. A write that fails leaves nothing behind: the
/// partly written file would make `scan --continue` find a page file the journal does not keep.
fn create_page_file(path: &Path, bytes: &[u8], write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if let Err(error) = write(&mut file, bytes) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(format!("{}: {error}", path.display()));
    }
    Ok(())
}

/// Write pages `*written..` of the job's held pages into the folder, never over an existing file. `base` is the
/// number of pages the job kept before this run.
fn flush_pages(
    job: &ScanJob,
    folder: &Path,
    base: usize,
    duplex: Option<ManualDuplex>,
    written: &mut usize,
    saved: &mut Vec<Value>,
) -> Result<(), String> {
    use std::io::Write;
    for (index, page) in job.pages().iter().enumerate().skip(*written) {
        let path = scan::page_path(folder, page_number(duplex, base + index), page.format);
        create_page_file(&path, &page.bytes, |file, bytes| file.write_all(bytes))?;
        eprintln!("pdfcraft-cli: saved {}", path.display());
        let mut row = crate::page_json(&path, page);
        if let Some(duplex) = duplex {
            row["side"] = json!(if base + index < usize::try_from(duplex.sheets).unwrap_or(usize::MAX) { "front" } else { "back" });
        }
        saved.push(row);
        *written = index + 1;
    }
    Ok(())
}

/// A job error as a plain `scan: ...` CLI error (exit status 1).
fn job_error(error: &ScanJobError) -> CliError {
    CliError::from(format!("scan: {error}"))
}

/// Run the job to its end (or to the point where a person must act) and exit by its class.
pub fn run(spec: Run) -> Result<(), CliError> {
    crate::ensure_scanning_supported()?;
    let Run { folder, settings, resume, duplex } = spec;
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
    let journal = Journal { folder: &folder, id, started, request: scan_jobs::request_json(&settings), prior_attempts, duplex };

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
                if write_error.is_none()
                    && let Err(error) = flush_pages(&job, &folder, base, duplex, &mut written, &mut saved)
                {
                    // Out of space or a file in the way: stop the transfer rather than pull more paper.
                    write_error = Some(error);
                    job.cancel(now());
                }
                // The job file cannot be written (disk full, folder gone): stop the transfer like a failed page write, and let
                // the loop join the worker; returning here would end the process with the scanner still feeding.
                if let Err(error) = journal.save(&job)
                    && write_error.is_none()
                {
                    write_error = Some(error.to_string());
                    job.cancel(now());
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let result = worker.join().unwrap_or_else(|_| Err(ScanError::Failed("the scan worker stopped unexpectedly".into())));
        job.finish_attempt(result, now()).map_err(|error| job_error(&error))?;
        if write_error.is_none()
            && let Err(error) = flush_pages(&job, &folder, base, duplex, &mut written, &mut saved)
        {
            write_error = Some(error);
        }
        // The worker has ended: a failed write is reported with the pages saved so far, not by dropping the result.
        if let Err(error) = journal.save(&job)
            && write_error.is_none()
        {
            write_error = Some(error.to_string());
        }
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
    let position = duplex.map(|duplex| duplex.after(job.page_count()));
    let class = if write_error.is_some() { ExitClass::Failed } else { class };
    let code = scan_jobs::exit_code(class);
    let source = snapshot.source_in_use.map_or(settings.source.name(), |source| source.name());
    let result = json!({
        "source": source,
        "requested_source": settings.source.name(),
        "reverse": duplex.is_some(),
        "manual_duplex": position.map_or(Value::Null, |p| json!({ "sheets": p.sheets, "direction": p.direction.as_str(), "cursor": p.cursor })),
        "count": saved.len(),
        "pages": saved,
        "state": state.name(),
        "exit_class": class.name(),
        "exit_code": code,
        "journal": folder.join(scan_jobs::JOB_FILE).display().to_string(),
        "job": serde_json::to_value(job.view()).unwrap_or(Value::Null),
    });
    stdout_line(format_args!("{}", serde_json::to_string(&result).map_err(|error| format!("scan: {error}"))?))?;
    let d = folder.display();
    if code == 0 {
        // A whole-feeder job may be one load of a larger pile, and its backs may follow.
        if duplex.is_none() && settings.pages == 0 {
            if settings.duplex {
                // The duplex unit scanned both sides already: more sheets only.
                eprintln!("pdfcraft-cli: more sheets: scan --continue {d} --append --yes");
            } else {
                eprintln!(
                    "pdfcraft-cli: more sheets: scan --continue {d} --append --yes; the backs: flip the whole stack, then scan --continue {d} --reverse --yes"
                );
            }
        }
        return Ok(());
    }
    let (message, json) = match (&write_error, &snapshot.last_error) {
        (Some(error), _) => (format!("scan: could not save the scan: {error}"), None),
        (None, Some(failure)) => {
            let info = error_line(&failure.info);
            (format!("scan: {}", failure.info.message), Some(info))
        }
        (None, None) => (format!("scan: {}", snapshot.summary_line()), None),
    };
    let hint = match (class, position) {
        (ExitClass::NeedsAttention | ExitClass::Failed, Some(position)) if position.cursor > 0 => format!(
            "\n  job: {}\n  backs: put the next part of the flipped stack in the feeder (the back of sheet {} is next), then scan --continue {d} --append --yes",
            snapshot.summary_line(),
            position.cursor
        ),
        (ExitClass::NeedsAttention | ExitClass::Failed, _) => format!("\n  job: {}\n  see: pdfcraft-cli scan job {d}", snapshot.summary_line()),
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
    /// `--reverse`: the fronts are all in; flip the whole stack and scan the backs from the last sheet.
    pub reverse: bool,
}

/// What `--continue` will do.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// Go on with an unfinished job: the person's choice for the kept pages. For a manual-duplex job in its backs
    /// pass, `Restart` scans the backs again and keeps the fronts.
    Continue(Continuation),
    /// A finished whole-feeder job: scan the next feeder load as more sheets.
    AddSheets,
    /// A finished single-sided job: turn it for its backs.
    Turn,
}

/// Apply the continuation rules. `Ok(plan)` is what to do; an `Err` is the sentence to show, and nothing has been
/// touched.
pub fn continuation_for(record: &JobRecord, files_on_disk: Result<usize, String>, flags: &ContinueFlags) -> Result<Plan, String> {
    if flags.append && flags.restart {
        return Err("scan --continue: give --append or --restart, not both".to_string());
    }
    if flags.reverse && (flags.append || flags.restart) {
        return Err("scan --continue: --reverse starts the backs; give it alone (later parts of the flipped stack use --append)".to_string());
    }
    let state = record.effective_state();
    let kept = record.pages_kept();
    let on_disk = files_on_disk.map_err(|reason| format!("scan --continue: {reason}"))?;
    if on_disk != kept {
        return Err(format!(
            "scan --continue: the journal keeps {kept} page(s) but {on_disk} page file(s) are in the folder; the journal does not verify files, so fix the folder (or start over in a new one) before continuing"
        ));
    }
    if flags.reverse {
        if record.can_turn() {
            return Ok(Plan::Turn);
        }
        return Err(if record.manual_duplex.is_some() {
            "scan --continue: the backs of this job are already being scanned; put the next part of the flipped stack in the feeder and use --append"
                .to_string()
        } else if record.two_sided() {
            "scan --continue: this job was scanned on both sides by the scanner's duplex unit; there are no reverse sides to add (--append adds more sheets)"
                .to_string()
        } else {
            "scan --continue: --reverse needs the fronts to be finished first (this job is not complete)".to_string()
        });
    }
    if state == ScanJobState::Terminal(Outcome::Completed) {
        if flags.append && record.can_add_sheets() {
            return Ok(Plan::AddSheets);
        }
        // A backs pass that ran the feeder empty ended Completed with backs still missing: --append scans the next
        // part of the flipped stack, as it does for an interrupted backs pass.
        if flags.append && record.backs_remaining() {
            return Ok(Plan::Continue(Continuation::Append));
        }
        if record.backs_remaining() {
            return Err(
                "scan --continue: the backs are not finished; put the next part of the flipped stack in the feeder and use --append".to_string()
            );
        }
        let more = if record.can_add_sheets() { "; --append adds the next feeder load" } else { "" };
        let backs = if record.can_turn() { "; --reverse scans the backs (flip the whole stack first)" } else { "" };
        return Err(format!("scan --continue: this job is complete{more}{backs}"));
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
        return Ok(Plan::Continue(Continuation::Restart));
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
            Ok(Plan::Continue(Continuation::Append))
        }
        (_, true) => Ok(Plan::Continue(Continuation::Restart)),
        _ => Err(match record.manual_duplex {
            Some(duplex) => format!(
                "scan --continue: {} back(s) are kept, so say what to do with them:\n  --append   keep them and scan the next part of the flipped stack (the back of sheet {} is next)\n  --restart  scan the backs again; the kept backs are moved aside into discarded-N, the fronts stay",
                duplex.backs_scanned(),
                duplex.cursor
            ),
            None => format!(
                "scan --continue: {kept} page(s) are kept, so say what to do with them:\n  --append   keep them and scan only the missing ones (put the remaining sheets in the feeder)\n  --restart  scan everything again; the kept page files are moved aside into discarded-N, not deleted"
            ),
        }),
    }
}

/// Move the kept page files (and a copy of the journal) into `DIR/discarded-<n>`; never deletes anything.
pub fn move_aside(folder: &Path) -> Result<PathBuf, String> {
    let files = scan::listed_page_files(folder).map_err(|error| error.to_string())?;
    move_files_aside(folder, files)
}

/// The backs of a manual-duplex job (the even page files) into `DIR/discarded-<n>`; the fronts stay.
pub fn move_backs_aside(folder: &Path) -> Result<PathBuf, String> {
    let backs =
        scan_jobs::page_numbers(folder)?.into_iter().filter(|number| number % 2 == 0).filter_map(|number| find_page(folder, number)).collect();
    move_files_aside(folder, backs)
}

/// The page file for page `number` (`page-NNN.<extension>`), whatever its extension, or `None` when the folder
/// has none or cannot be read.
fn find_page(folder: &Path, number: usize) -> Option<PathBuf> {
    let prefix = format!("page-{number:03}.");
    std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix)))
}

/// Move `files` and a copy of the job file into a new `discarded-<n>` folder (the first number not in use)
/// inside `folder`, and return that folder. Nothing is deleted.
fn move_files_aside(folder: &Path, files: Vec<PathBuf>) -> Result<PathBuf, String> {
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
    let fail = |error: String| CliError::from(format!("scan --continue: {error}"));
    let mut record = scan_jobs::read_record(folder).map_err(fail)?;
    if let Some(duplex) = record.manual_duplex {
        // A turn that was cut off has its fronts placed first (a no-op when they already are), so the folder
        // can be checked against the journal.
        scan_jobs::place_fronts_for_duplex(folder, duplex.sheets).map_err(fail)?;
    }
    let on_disk = scan_jobs::page_files_for(folder, &record);
    let plan = continuation_for(&record, on_disk, flags)?;
    let mut settings = scan_jobs::settings_from_request(&record.request).map_err(fail)?;
    settings.checked().map_err(|error| fail(error.to_string()))?;
    crate::ensure_scanning_supported()?;
    let folder_buf = folder.to_path_buf();
    match plan {
        Plan::AddSheets => run(Run { folder: folder_buf, settings, resume: Some((record, Continuation::Append)), duplex: None }),
        Plan::Turn => {
            // One job, now twice as long: the fronts are kept and the backs follow from the last sheet down. The
            // journal says so before any file moves, so a turn that is cut off is finished by the next --continue.
            let sheets = u32::try_from(record.pages_kept()).map_err(|_| fail("too many pages".into()))?;
            let duplex = ManualDuplex::backs_of(sheets);
            settings.pages = sheets.saturating_mul(2);
            record.request = scan_jobs::request_json(&settings);
            record.journal.expected_pages = Some(settings.pages);
            record.journal.state = ScanJobState::Interrupted;
            record.manual_duplex = Some(duplex);
            record.updated = scan_jobs::now_millis();
            scan_jobs::write_record(folder, &record).map_err(fail)?;
            scan_jobs::place_fronts_for_duplex(folder, sheets).map_err(fail)?;
            eprintln!("pdfcraft-cli: the fronts are now the odd pages; scanning the backs from sheet {sheets} down");
            run(Run { folder: folder_buf, settings, resume: Some((record, Continuation::Append)), duplex: Some(duplex) })
        }
        Plan::Continue(how) => match record.manual_duplex {
            Some(duplex) => {
                // The job is the whole stack, both sides: that, not the request of the fronts, is what it wants.
                settings.pages = duplex.sheets.saturating_mul(2);
                if how == Continuation::Restart && duplex.backs_scanned() > 0 {
                    let aside = move_backs_aside(folder).map_err(fail)?;
                    eprintln!("pdfcraft-cli: moved the backs aside into {}; the fronts stay", aside.display());
                    record.journal.pages.truncate(usize::try_from(duplex.sheets).unwrap_or(usize::MAX));
                }
                run(Run { folder: folder_buf, settings, resume: Some((record, Continuation::Append)), duplex: Some(duplex) })
            }
            None => {
                if how == Continuation::Restart && record.pages_kept() > 0 {
                    let aside = move_aside(folder).map_err(fail)?;
                    eprintln!("pdfcraft-cli: moved the kept pages aside into {}", aside.display());
                }
                run(Run { folder: folder_buf, settings, resume: Some((record, how)), duplex: None })
            }
        },
    }
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
    use pdfcraft_platform::scan::Page;
    use pdfcraft_platform::scan::job::ScanJobJournal;

    #[test]
    fn a_page_file_whose_write_fails_is_removed() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-scan-partial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("page-001.bmp");
        let failed = create_page_file(&path, b"0123456789", |file, bytes| {
            use std::io::Write;
            file.write_all(&bytes[..4])?;
            Err(std::io::Error::other("disk full"))
        });
        assert!(failed.unwrap_err().contains("disk full"));
        assert!(!path.exists(), "a partly written page was left in the folder");
        // A good write keeps the file, and an existing file is never written over.
        create_page_file(&path, b"01234567", std::io::Write::write_all).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"01234567");
        assert!(create_page_file(&path, b"x", std::io::Write::write_all).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"01234567");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A journal record in `state` that holds `kept` saved 2x2 pages and asked for `expected` pages (None: all).
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
            request: scan_jobs::request_json(&settings),
            journal,
            view: Value::Null,
            attempts: Vec::new(),
            manual_duplex: None,
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

    const NONE: ContinueFlags = ContinueFlags { append: false, restart: false, checked: false, reverse: false };

    #[test]
    fn continuing_with_pages_kept_needs_an_explicit_choice() {
        let record = record(ScanJobState::Interrupted, 2, Some(5));
        let message = continuation_for(&record, Ok(2), &NONE).unwrap_err();
        assert!(message.contains("--append") && message.contains("--restart"), "{message}");
        assert_eq!(continuation_for(&record, Ok(2), &ContinueFlags { append: true, ..NONE }), Ok(Plan::Continue(Continuation::Append)));
        assert_eq!(continuation_for(&record, Ok(2), &ContinueFlags { restart: true, ..NONE }), Ok(Plan::Continue(Continuation::Restart)));
        let both = ContinueFlags { append: true, restart: true, checked: false, reverse: false };
        assert!(continuation_for(&record, Ok(2), &both).unwrap_err().contains("not both"));
    }

    #[test]
    fn continuing_with_nothing_kept_needs_no_choice() {
        let record = record(ScanJobState::Terminal(Outcome::Failed), 0, Some(5));
        assert_eq!(continuation_for(&record, Ok(0), &NONE), Ok(Plan::Continue(Continuation::Restart)));
    }

    #[test]
    fn needs_attention_is_refused_without_the_acknowledgement() {
        let record = record(ScanJobState::NeedsAttention, 2, Some(5));
        let flags = ContinueFlags { append: true, ..NONE };
        let message = continuation_for(&record, Ok(2), &flags).unwrap_err();
        assert!(message.contains("--i-checked-the-scanner") && message.contains("carriage"), "{message}");
        let checked = ContinueFlags { append: true, checked: true, restart: false, reverse: false };
        assert_eq!(continuation_for(&record, Ok(2), &checked), Ok(Plan::Continue(Continuation::Append)));
    }

    #[test]
    fn a_job_cut_off_while_acquiring_counts_as_needs_attention() {
        let record = record(ScanJobState::Acquiring, 0, Some(5));
        let message = continuation_for(&record, Ok(0), &NONE).unwrap_err();
        assert!(message.contains("ended without finishing"), "{message}");
        let checked = ContinueFlags { checked: true, ..NONE };
        assert_eq!(continuation_for(&record, Ok(0), &checked), Ok(Plan::Continue(Continuation::Restart)));
    }

    #[test]
    fn a_complete_job_and_a_changed_folder_are_refused() {
        let done = record(ScanJobState::Terminal(Outcome::Completed), 5, Some(5));
        let message = continuation_for(&done, Ok(5), &NONE).unwrap_err();
        assert!(message.contains("complete") && message.contains("--reverse") && !message.contains("--append"), "{message}");
        assert!(continuation_for(&done, Ok(5), &ContinueFlags { append: true, ..NONE }).is_err(), "a counted job does not grow");
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
        assert_eq!(continuation_for(&record, Ok(5), &ContinueFlags { restart: true, ..NONE }), Ok(Plan::Continue(Continuation::Restart)));
    }

    #[test]
    fn a_finished_whole_feeder_job_takes_more_sheets_or_turns_for_its_backs() {
        let done = record(ScanJobState::Terminal(Outcome::Completed), 3, None);
        assert_eq!(continuation_for(&done, Ok(3), &ContinueFlags { append: true, ..NONE }), Ok(Plan::AddSheets));
        assert_eq!(continuation_for(&done, Ok(3), &ContinueFlags { reverse: true, ..NONE }), Ok(Plan::Turn));
        let alone = continuation_for(&done, Ok(3), &ContinueFlags { reverse: true, append: true, ..NONE }).unwrap_err();
        assert!(alone.contains("give it alone"), "{alone}");
        let unfinished = record(ScanJobState::Interrupted, 2, None);
        assert!(continuation_for(&unfinished, Ok(2), &ContinueFlags { reverse: true, ..NONE }).unwrap_err().contains("finished first"));
        assert!(continuation_for(&done, Ok(2), &ContinueFlags { reverse: true, ..NONE }).unwrap_err().contains("2 page file(s)"));
    }

    #[test]
    fn a_job_scanned_through_the_duplex_unit_takes_more_sheets_but_never_turns() {
        let mut two_sided = record(ScanJobState::Terminal(Outcome::Completed), 4, None);
        two_sided.request = scan_jobs::request_json(&ScanSettings { duplex: true, ..ScanSettings::default() });
        assert_eq!(continuation_for(&two_sided, Ok(4), &ContinueFlags { append: true, ..NONE }), Ok(Plan::AddSheets));
        let refused = continuation_for(&two_sided, Ok(4), &ContinueFlags { reverse: true, ..NONE }).unwrap_err();
        assert!(refused.contains("duplex unit") && refused.contains("--append"), "{refused}");
        let message = continuation_for(&two_sided, Ok(4), &NONE).unwrap_err();
        assert!(!message.contains("--reverse"), "no backs to offer: {message}");
    }

    #[test]
    fn the_backs_pass_continues_down_the_stack() {
        let mut backs = record(ScanJobState::Interrupted, 4, Some(6));
        backs.manual_duplex = Some(ManualDuplex::backs_of(3).after(4));
        assert_eq!(continuation_for(&backs, Ok(4), &ContinueFlags { append: true, ..NONE }), Ok(Plan::Continue(Continuation::Append)));
        assert_eq!(continuation_for(&backs, Ok(4), &ContinueFlags { restart: true, ..NONE }), Ok(Plan::Continue(Continuation::Restart)));
        let choose = continuation_for(&backs, Ok(4), &NONE).unwrap_err();
        assert!(
            choose.contains("1 back(s) are kept") && choose.contains("the back of sheet 2 is next") && choose.contains("the fronts stay"),
            "{choose}"
        );
        let again = continuation_for(&backs, Ok(4), &ContinueFlags { reverse: true, ..NONE }).unwrap_err();
        assert!(again.contains("already being scanned"), "{again}");
    }

    #[test]
    fn a_backs_pass_that_ran_the_feeder_empty_continues_with_append() {
        // A whole-feeder backs pass ends Completed when the part of the stack in the feeder runs out.
        let mut backs = record(ScanJobState::Terminal(Outcome::Completed), 4, None);
        backs.manual_duplex = Some(ManualDuplex::backs_of(3).after(4));
        assert_eq!(continuation_for(&backs, Ok(4), &ContinueFlags { append: true, ..NONE }), Ok(Plan::Continue(Continuation::Append)));
        let told = continuation_for(&backs, Ok(4), &NONE).unwrap_err();
        assert!(told.contains("backs are not finished") && told.contains("--append"), "{told}");
        // Every back in: complete, nothing to append.
        backs.manual_duplex = Some(ManualDuplex::backs_of(3).after(6));
        assert!(continuation_for(&backs, Ok(4), &ContinueFlags { append: true, ..NONE }).unwrap_err().contains("complete"));
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
        /// The feeder holds this many sheets: they arrive and the transfer ends normally (a whole-feeder scan).
        Load(usize),
        /// This many pages arrive, then the feeder runs empty before the count asked for.
        EmptyAfter(usize),
        /// This many pages arrive, then the feeder jams.
        JamAfter(usize),
        /// The link drops with no page in hand.
        Offline,
        /// All pages arrive, but first the job file in this folder is replaced by a directory, so the next write of it fails.
        /// The transfer then stays open until it is cancelled (or five seconds pass), and records that it ended.
        BreakTheJournal(PathBuf),
    }

    static SCRIPT: Mutex<VecDeque<Step>> = Mutex::new(VecDeque::new());
    static ASKED: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    /// Sheets fed so far in a test: each page's bytes are its feed number, so tests can see where it landed.
    static FED: Mutex<u8> = Mutex::new(0);
    static SERIAL: Mutex<()> = Mutex::new(());
    /// Set by the mock when its transfer has ended: a run that returns while this is still false left the scanner feeding.
    static WORKER_ENDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    struct Mock;

    /// A 2x2 page whose bytes are the running number of pages fed so far, so a test can tell where it ended up.
    fn page(_: u8) -> Page {
        let fed = {
            let mut fed = FED.lock().unwrap();
            *fed += 1;
            *fed
        };
        Page {
            bytes: vec![fed; 8],
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
                Step::Load(n) => {
                    let pages: Vec<Page> = (0..n).map(|k| page(k as u8)).collect();
                    for page in &pages {
                        progress.push_page(page.clone());
                    }
                    Ok(Acquired { source: Source::Feeder, pages })
                }
                Step::EmptyAfter(n) => {
                    let pages: Vec<Page> = (0..n).map(|k| page(k as u8)).collect();
                    for page in &pages {
                        progress.push_page(page.clone());
                    }
                    Err(ScanError::Incomplete {
                        message: "The feeder ran out of paper.".into(),
                        acquired: Acquired { source: Source::Feeder, pages },
                        cause: Some(Cause { kind: InterruptKind::FeederEmpty, native: None }),
                    })
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
                Step::BreakTheJournal(folder) => {
                    let job_file = folder.join(scan_jobs::JOB_FILE);
                    let _ = std::fs::remove_file(&job_file);
                    std::fs::create_dir(&job_file).expect("a directory where the job file was");
                    for _ in 0..wanted {
                        progress.push_page(page(0));
                    }
                    let cancel = settings.cancel.clone().expect("the job gives every attempt a cancel token");
                    let waited = std::time::Instant::now();
                    while !cancel.is_requested() && waited.elapsed() < Duration::from_secs(5) {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    WORKER_ENDED.store(true, std::sync::atomic::Ordering::SeqCst);
                    Err(ScanError::Native { kind: InterruptKind::Cancelled, native: None, message: "The scan was cancelled.".into() })
                }
            }
        }
    }

    /// Register the mock backend once, take the lock that serialises tests (it tolerates a poisoned lock), and reset the
    /// script, the page requests and the feed counter.
    fn mock_lock() -> MutexGuard<'static, ()> {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let _ = scan::register(Box::new(Mock));
        });
        let guard = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        SCRIPT.lock().unwrap().clear();
        ASKED.lock().unwrap().clear();
        *FED.lock().unwrap() = 0;
        WORKER_ENDED.store(false, std::sync::atomic::Ordering::SeqCst);
        guard
    }

    /// Queue the steps the mock backend plays, one per `acquire` call.
    fn script(steps: Vec<Step>) {
        *SCRIPT.lock().unwrap() = steps.into();
    }

    /// A new empty folder for a test, named by `name` and the process id.
    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-scan-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Run a fresh feeder job of `pages` pages into `folder` against the mock backend.
    fn start(folder: &Path, pages: u32) -> Result<(), CliError> {
        let settings = ScanSettings { device: "mock:1".into(), source: scan::Source::Feeder, pages, ..ScanSettings::default() };
        run(Run { folder: folder.to_path_buf(), settings, resume: None, duplex: None })
    }

    /// The exit status of a finished job: 0 for success, the job's code for `CliError::Exit`; any other error panics.
    fn exit_code_of(result: Result<(), CliError>) -> u8 {
        match result {
            Ok(()) => 0,
            Err(CliError::Exit { code, .. }) => code,
            Err(other) => panic!("not a job exit: {other}"),
        }
    }

    /// The sorted names of the entries in `dir`.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    /// The message with which a fresh scan into `dir` is refused.
    fn refuse(dir: &Path) -> String {
        crate::refuse_a_folder_that_has_a_job(dir).unwrap_err().to_string()
    }

    #[test]
    fn a_job_file_that_cannot_be_written_stops_the_transfer_and_ends_the_worker_before_run_returns() {
        let _serial = mock_lock();
        let dir = fresh("journal-write-fails");
        script(vec![Step::BreakTheJournal(dir.clone())]);
        let result = start(&dir, 3);
        // Returning on the failed write used to end the process with the transfer still running.
        assert!(WORKER_ENDED.load(std::sync::atomic::Ordering::SeqCst), "run returned while the scanner was still feeding");
        match result {
            Err(CliError::Exit { code, message, .. }) => {
                assert_ne!(code, 0);
                assert!(message.contains("could not save the scan"), "{message}");
            }
            other => panic!("expected a failed job, got {other:?}"),
        }
        // The pages that arrived before the failure are on disk.
        assert!(names(&dir).iter().any(|n| n.starts_with("page-")), "{:?}", names(&dir));
        let _ = std::fs::remove_dir_all(&dir);
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
        let checked = ContinueFlags { append: true, checked: true, restart: false, reverse: false };
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

    const APPEND: ContinueFlags = ContinueFlags { append: true, ..NONE };
    const REVERSE: ContinueFlags = ContinueFlags { reverse: true, ..NONE };

    /// The feed number of each page file, in page order.
    fn feed_order(dir: &Path) -> Vec<u8> {
        scan::listed_page_files(dir).unwrap().iter().map(|path| std::fs::read(path).unwrap()[0]).collect()
    }

    #[test]
    fn a_large_pile_goes_in_as_batches_of_fronts_then_batches_of_backs_down_the_stack() {
        let _serial = mock_lock();
        let dir = fresh("pile");
        // Fronts in two feeder loads: sheets 1-2, then sheet 3.
        script(vec![Step::Load(2)]);
        assert_eq!(exit_code_of(start(&dir, 0)), 0);
        script(vec![Step::Load(1)]);
        assert_eq!(exit_code_of(continue_job(&dir, &APPEND)), 0);
        let record = scan_jobs::read_record(&dir).unwrap();
        assert_eq!((record.pages_kept(), record.journal.state), (3, ScanJobState::Terminal(Outcome::Completed)));
        assert_eq!(feed_order(&dir), [1, 2, 3]);
        // Turn: the whole stack flipped; the first part of it holds the backs of sheets 3 and 2, then the feeder is empty.
        script(vec![Step::EmptyAfter(2)]);
        assert_eq!(exit_code_of(continue_job(&dir, &REVERSE)), 10, "a part of the stack is done; the rest needs loading");
        let record = scan_jobs::read_record(&dir).unwrap();
        assert_eq!(record.manual_duplex, Some(ManualDuplex { sheets: 3, direction: scan_jobs::Direction::Backs, cursor: 1 }));
        assert_eq!((record.pages_kept(), record.journal.expected_pages), (5, Some(6)));
        assert_eq!(scan_jobs::page_numbers(&dir).unwrap(), [1, 3, 4, 5, 6], "every page is already in its place");
        assert!(scan_jobs::page_mismatch(&dir, &record).is_none());
        assert!(scan_jobs::detail_text("pdfcraft-cli", &dir, &record).contains("the back of sheet 1 is next"));
        // The next part of the flipped stack: the back of sheet 1.
        script(vec![Step::Complete]);
        assert_eq!(exit_code_of(continue_job(&dir, &APPEND)), 0);
        assert_eq!(*ASKED.lock().unwrap(), [0, 0, 3, 1], "each pass asks only for what is left");
        // Fronts 1, 2, 3 were fed first, then the backs of sheets 3, 2, 1 as feeds 4, 5, 6.
        assert_eq!(feed_order(&dir), [1, 6, 2, 5, 3, 4]);
        let record = scan_jobs::read_record(&dir).unwrap();
        assert_eq!(record.journal.state, ScanJobState::Terminal(Outcome::Completed));
        assert_eq!(record.manual_duplex.map(|d| d.cursor), Some(0));
        let numbers: Vec<u32> = record.journal.pages.iter().map(|page| page.number).collect();
        assert_eq!(numbers, [1, 3, 5, 6, 4, 2], "each page record names its file: fronts, then the backs from sheet 3 down");
        assert!(scan_jobs::page_mismatch(&dir, &record).is_none());
        assert_eq!(record.attempts.len(), 4, "one job, one timeline");
        assert!(continuation_for(&record, Ok(6), &APPEND).unwrap_err().contains("complete"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restarting_the_backs_moves_only_the_backs_aside() {
        let _serial = mock_lock();
        let dir = fresh("backs-restart");
        script(vec![Step::Complete]);
        assert_eq!(exit_code_of(start(&dir, 3)), 0);
        script(vec![Step::JamAfter(1)]);
        assert_eq!(exit_code_of(continue_job(&dir, &REVERSE)), 10);
        assert_eq!(scan_jobs::page_numbers(&dir).unwrap(), [1, 3, 5, 6]);
        script(vec![Step::Complete]);
        assert_eq!(exit_code_of(continue_job(&dir, &ContinueFlags { restart: true, ..NONE })), 0);
        assert_eq!(names(&dir.join("discarded-1")), ["page-006.bmp", "scan-job.json"]);
        assert_eq!(*ASKED.lock().unwrap(), [3, 3, 3], "the restart asks for every back again, not for the fronts");
        assert_eq!(feed_order(&dir), [1, 7, 2, 6, 3, 5], "fronts 1-3 kept; the discarded back was feed 4");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_turn_cut_off_while_placing_the_fronts_is_finished_by_the_next_continue() {
        let _serial = mock_lock();
        let dir = fresh("turn-cut");
        script(vec![Step::Complete]);
        assert_eq!(exit_code_of(start(&dir, 3)), 0);
        // What the turn writes before it moves anything, and the first move only.
        let mut record = scan_jobs::read_record(&dir).unwrap();
        record.manual_duplex = Some(ManualDuplex::backs_of(3));
        record.journal.expected_pages = Some(6);
        record.journal.state = ScanJobState::Interrupted;
        scan_jobs::write_record(&dir, &record).unwrap();
        std::fs::rename(dir.join("page-003.bmp"), dir.join("page-005.bmp")).unwrap();
        script(vec![Step::Complete]);
        assert_eq!(exit_code_of(continue_job(&dir, &APPEND)), 0);
        assert_eq!(feed_order(&dir), [1, 6, 2, 5, 3, 4]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fresh_scan_with_reverse_points_at_continue() {
        let message = crate::scan(&["--out".into(), "pages".into(), "--yes".into(), "--reverse".into()]).unwrap_err().to_string();
        assert!(message.contains("scan --continue pages --reverse --yes"), "{message}");
    }
}
