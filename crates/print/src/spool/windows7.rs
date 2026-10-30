//! The experimental Windows 7 build's spooler (docs/windows7.md). craft-print draws jobs with
//! `Windows.Data.Pdf`, a Windows Runtime API Windows 7 doesn't have, so this build hands the
//! print-ready PDF to the installed PDF viewer's Print / PrintTo verb instead, as VectorCraft's
//! Windows 7 build does. The printer list still comes from the spooler (EnumPrinters, through
//! craft-print's Win32 `rgb` module). The viewer prints with its own settings: copies, two-sided
//! and scaling chosen here don't reach it, and the result says so.

use super::Job;
#[cfg(target_vendor = "win7")]
use super::Printer;
#[cfg(target_vendor = "win7")]
use crate::PrintError;

/// Where a job's PDF waits for the viewer: a fresh file in `dir`. The viewer reads it after the
/// verb returns, so it can't be removed then; files of jobs more than an hour old are removed
/// first instead. Never truncates an existing file, and removes one it couldn't finish.
pub(crate) fn job_file(dir: &std::path::Path, pdf: &[u8]) -> Result<std::path::PathBuf, String> {
    use std::io::Write as _;
    std::fs::create_dir_all(dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    sweep(dir);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let path = dir.join(format!("job-{}-{stamp}.pdf", std::process::id()));
    let mut file =
        std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| format!("can't create {}: {e}", path.display()))?;
    if let Err(e) = file.write_all(pdf).and_then(|()| file.flush()) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(format!("can't write {}: {e}", path.display()));
    }
    Ok(path)
}

/// Removes the job files in `dir` more than an hour old: their viewer has long read them. Runs
/// before each job and whenever the printers are listed (the Print dialog opens), so a job's file
/// outlives it by about an hour of using PdfCraft's printing. A file still in use stays for the
/// next sweep; the folder is the user's own temp folder.
pub(crate) fn sweep(dir: &std::path::Path) {
    // A folder of a few files; the cap only bounds a runaway listing.
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten().take(4096) {
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 3600);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The environment variables that carry the job's file and printer into PowerShell. A value in
/// the environment is data to PowerShell, never parsed as script, whatever quotes it contains
/// (PowerShell also ends a single-quoted string at ‘ ’ ‚ ‛), so neither is ever written into the
/// script's source.
const FILE_VAR: &str = "PDFCRAFT_PRINT_FILE";
const PRINTER_VAR: &str = "PDFCRAFT_PRINT_PRINTER";

/// The fixed scripts: Print on the default printer, PrintTo on a named one. Each writes `ok`
/// once the viewer has been started; `$ErrorActionPreference = 'Stop'` turns a failure into no `ok`.
const PRINT_SCRIPT: &str = "Start-Process -FilePath $env:PDFCRAFT_PRINT_FILE -Verb Print -WindowStyle Hidden; 'ok'";
const PRINT_TO_SCRIPT: &str =
    "Start-Process -FilePath $env:PDFCRAFT_PRINT_FILE -Verb PrintTo -ArgumentList $env:PDFCRAFT_PRINT_PRINTER -WindowStyle Hidden; 'ok'";

/// The script and environment that print `file` on `printer` (the default printer when `None` or
/// blank) through the PDF viewer's verb. PrintTo takes the printer name in double quotes, which a
/// Windows printer name can't contain.
pub(crate) fn print_invocation(file: &str, printer: Option<&str>) -> (&'static str, Vec<(&'static str, String)>) {
    let mut env = vec![(FILE_VAR, file.to_string())];
    match printer.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => {
            env.push((PRINTER_VAR, format!("\"{}\"", p.replace('"', ""))));
            (PRINT_TO_SCRIPT, env)
        }
        None => (PRINT_SCRIPT, env),
    }
}

/// How long PowerShell may take to start the viewer before the job is given up: a viewer whose
/// print handler hangs must not leave the job pending for ever.
#[cfg(target_vendor = "win7")]
const LAUNCH_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// What the dialog is told once the viewer has the job: the settings the viewer decides.
pub(crate) fn handed_over(job: &Job) -> String {
    let mut note = String::from("handed to the PDF viewer's print command, which prints with its own settings");
    if job.copies > 1 {
        note.push_str(&format!(" (one copy, not {})", job.copies));
    }
    note
}

/// Windows PowerShell (2.0 on Windows 7 SP1) running `script`, without a console window.
#[cfg(target_vendor = "win7")]
fn powershell(script: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let root = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()));
    let mut c = std::process::Command::new(root.join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe"));
    c.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &format!("$ErrorActionPreference = 'Stop'; [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false); {script}"),
    ])
    .creation_flags(CREATE_NO_WINDOW);
    c
}

/// The spooler's printers, the default first; empty when it can't be asked.
#[cfg(target_vendor = "win7")]
pub fn printers() -> Vec<Printer> {
    sweep(&std::env::temp_dir().join("pdfcraft-print"));
    let list = craft_print::rgb::printers().unwrap_or_default();
    list.into_iter().map(|p| Printer { name: p.name, description: String::new(), default: p.default, two_sided: None }).collect()
}

/// Print `pdf` through the PDF viewer's verb. Progress is reported once, when the viewer has it.
#[cfg(target_vendor = "win7")]
pub fn submit_with_progress(pdf: &[u8], job: &Job, progress: &mut dyn FnMut(usize, usize)) -> Result<String, PrintError> {
    let file = job_file(&std::env::temp_dir().join("pdfcraft-print"), pdf).map_err(PrintError::Spool)?;
    let (script, env) = print_invocation(&file.to_string_lossy(), job.printer.as_deref());
    let mut command = powershell(script);
    command.envs(env);
    let failed = match super::output_within(&mut command, LAUNCH_LIMIT) {
        Ok(out) if String::from_utf8_lossy(&out).trim() == "ok" => None,
        Ok(_) => Some("PowerShell couldn't start it".to_string()),
        Err(e) => Some(format!("PowerShell: {e}")),
    };
    if let Some(e) = failed {
        // No viewer took the job, so nothing will read the file.
        let _ = std::fs::remove_file(&file);
        return Err(PrintError::Spool(format!(
            "no app prints PDF files here; install a PDF viewer that can print, or save the print-ready PDF ({e})"
        )));
    }
    progress(1, 1);
    Ok(handed_over(job))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_and_printer_reach_powershell_as_data_never_as_script() {
        // Quotes PowerShell would end a single-quoted string at, ASCII and typographic.
        let hostile = "Office \"Laser\" 'B' ‘x’ ‚y‛; Remove-Item C:\\ $(calc)";
        let (script, env) = print_invocation(r"C:\Bob's ‘dir’\job.pdf", Some(hostile));
        assert_eq!(script, PRINT_TO_SCRIPT);
        assert!(!script.contains("Bob") && !script.contains("Laser"));
        assert_eq!(
            env,
            [(FILE_VAR, r"C:\Bob's ‘dir’\job.pdf".to_string()), (PRINTER_VAR, "\"Office Laser 'B' ‘x’ ‚y‛; Remove-Item C:\\ $(calc)\"".to_string())]
        );
        // The scripts read the variables they are given.
        assert!(PRINT_TO_SCRIPT.contains(&format!("$env:{FILE_VAR}")) && PRINT_TO_SCRIPT.contains(&format!("$env:{PRINTER_VAR}")));
        assert!(PRINT_SCRIPT.contains(&format!("$env:{FILE_VAR}")));
        for blank in [None, Some(""), Some("  ")] {
            assert_eq!(print_invocation("job.pdf", blank), (PRINT_SCRIPT, vec![(FILE_VAR, "job.pdf".to_string())]));
        }
    }

    #[test]
    fn the_sweep_removes_job_files_older_than_an_hour_only() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-print-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (old, new) = (dir.join("old.pdf"), dir.join("new.pdf"));
        std::fs::write(&old, b"a").unwrap();
        std::fs::write(&new, b"b").unwrap();
        let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(two_hours_ago).unwrap();
        sweep(&dir);
        assert!(!old.exists());
        assert!(new.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn job_files_hold_the_pdf_for_the_viewer_and_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-print-test-{}", std::process::id()));
        let a = job_file(&dir, b"%PDF-1.7 a").unwrap();
        let b = job_file(&dir, b"%PDF-1.7 b").unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read(&a).unwrap(), b"%PDF-1.7 a");
        assert_eq!(std::fs::read(&b).unwrap(), b"%PDF-1.7 b");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_result_says_the_viewer_decides_and_copies_are_not_applied() {
        let one = Job::default();
        assert!(!handed_over(&one).contains("copy"));
        let three = Job { copies: 3, ..Job::default() };
        assert!(handed_over(&three).ends_with("(one copy, not 3)"), "{}", handed_over(&three));
    }
}
