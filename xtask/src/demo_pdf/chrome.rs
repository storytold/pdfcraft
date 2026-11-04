//! Locating a Chromium-based browser and printing HTML to PDF with it.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// Find a Chrome/Chromium binary. `explicit` (from `--chrome`) wins, then the
/// `PDFCRAFT_CHROME` / `CHROME` environment variables, then well-known
/// install locations, then `PATH`.
pub fn find(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        bail!("--chrome {} does not exist", path.display());
    }
    for var in ["PDFCRAFT_CHROME", "CHROME"] {
        if let Some(path) = env::var_os(var).map(PathBuf::from)
            && path.is_file()
        {
            return Ok(path);
        }
    }

    let mut candidates: Vec<PathBuf> = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = env::var_os("HOME") {
        candidates.push(Path::new(&home).join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
        if let Some(base) = env::var_os(var) {
            let base = PathBuf::from(base);
            candidates.push(base.join(r"Google\Chrome\Application\chrome.exe"));
            candidates.push(base.join(r"Microsoft\Edge\Application\msedge.exe"));
        }
    }
    if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
        return Ok(found);
    }

    let names = ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome", "chrome.exe", "msedge"];
    let path = env::var_os("PATH").unwrap_or_default();
    for dir in env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!(
        "could not find Google Chrome or Chromium; install one or pass --chrome <path> \
         (or set PDFCRAFT_CHROME)"
    )
}

/// Print `html` to `pdf` with headless Chrome. Chrome emits a tagged PDF and,
/// with `--generate-pdf-document-outline`, bookmarks built from the headings.
///
/// Some Chrome builds finish writing the PDF but linger afterwards, so rather
/// than only waiting for exit we poll for a complete file and then stop Chrome.
pub fn print_to_pdf(chrome: &Path, html: &Path, pdf: &Path, log: &Path) -> Result<()> {
    if pdf.exists() {
        fs::remove_file(pdf).with_context(|| format!("removing stale {}", pdf.display()))?;
    }
    let mut print_arg = OsString::from("--print-to-pdf=");
    print_arg.push(pdf);
    let log_file = fs::File::create(log).with_context(|| format!("creating {}", log.display()))?;

    let profile = TempProfile::new();
    let mut child = Command::new(chrome)
        .args(chrome_args(&profile.0, print_arg, super::file_url(html)))
        .stdout(Stdio::null())
        .stderr(log_file)
        .spawn()
        .with_context(|| format!("running {}", chrome.display()))?;

    let started = Instant::now();
    let mut last_len = None;
    // Chrome's Skia back-end writes the PDF incrementally, and every increment ends with its own
    // `%%EOF`: a stable file that looks complete can still grow while later pages rasterise. Only
    // treat the file as finished after the size has been quiet for several consecutive polls.
    let mut quiet_polls = 0;
    loop {
        if let Some(status) = child.try_wait()? {
            // On Windows the launched `chrome.exe` can exit while its child processes are still
            // writing the PDF, so give the file up to 60 s to appear, and only accept it once the
            // size has been quiet and the file complete for 5 consecutive one-second polls.
            if wait_for_settled_pdf(pdf, Duration::from_secs(60), Duration::from_millis(1000), 5) {
                return Ok(());
            }
            bail!("Chrome exited with {status} without writing {}; see {}", pdf.display(), log.display());
        }
        if poll_settled_pdf(pdf, &mut last_len, &mut quiet_polls, 10) {
            stop(&mut child);
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(120) {
            stop(&mut child);
            bail!("Chrome timed out printing {}; see {}", html.display(), log.display());
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// Command-line arguments for a headless print. `MacAppCodeSignClone` is disabled because on macOS
/// every launch otherwise makes a code-sign clone of the app under `/private/var/folders`, and a
/// browser that is killed never removes it. The profile is a throwaway directory.
fn chrome_args(profile: &Path, print_arg: OsString, url: impl Into<OsString>) -> Vec<OsString> {
    let mut user_data = OsString::from("--user-data-dir=");
    user_data.push(profile);
    vec![
        "--headless=new".into(),
        "--disable-gpu".into(),
        "--disable-features=MacAppCodeSignClone".into(),
        user_data,
        "--no-pdf-header-footer".into(),
        "--generate-pdf-document-outline".into(),
        print_arg,
        url.into(),
    ]
}

/// Ask Chrome to quit (SIGTERM on Unix) so it can clean up after itself, and kill it only if it
/// is still running after a few seconds.
fn stop(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill").arg("-TERM").arg(child.id().to_string()).status();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// A throwaway Chrome profile directory, removed when dropped (also on error paths).
struct TempProfile(PathBuf);

impl TempProfile {
    fn new() -> Self {
        Self(env::temp_dir().join(format!("pdfcraft-chrome-profile-{}", std::process::id())))
    }
}

impl Drop for TempProfile {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A PDF is complete once its trailer's `%%EOF` marker has been written.
fn is_complete(pdf: &Path) -> bool {
    fs::read(pdf).map(|bytes| bytes.len() > 1024 && bytes[bytes.len() - 1024..].windows(5).any(|w| w == b"%%EOF")).unwrap_or(false)
}

/// Check whether `pdf` is complete and unchanged since `last_len`, updating `last_len` and
/// `quiet_polls`. Returns `true` once `quiet_polls` reaches `required_quiet_polls`.
fn poll_settled_pdf(pdf: &Path, last_len: &mut Option<u64>, quiet_polls: &mut usize, required_quiet_polls: usize) -> bool {
    let len = fs::metadata(pdf).map(|m| m.len()).ok();
    let settled = if len == *last_len && len.is_some() && is_complete(pdf) {
        *quiet_polls += 1;
        *quiet_polls >= required_quiet_polls.max(1)
    } else {
        *quiet_polls = 0;
        false
    };
    *last_len = len;
    settled
}

/// Poll `pdf` after the top-level Chrome process exits until the file exists, is complete, and
/// keeps the same byte length across `required_quiet_polls` consecutive polls of `poll_interval`,
/// or until `timeout` elapses.
fn wait_for_settled_pdf(pdf: &Path, timeout: Duration, poll_interval: Duration, required_quiet_polls: usize) -> bool {
    let deadline = Instant::now() + timeout;
    let mut last_len = None;
    let mut quiet_polls = 0;
    while Instant::now() < deadline {
        if poll_settled_pdf(pdf, &mut last_len, &mut quiet_polls, required_quiet_polls) {
            return true;
        }
        thread::sleep(poll_interval);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(tag: &str) -> Self {
            let dir = env::temp_dir().join(format!("pdfcraft-chrome-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn args_disable_code_sign_clone_and_use_throwaway_profile() {
        let args = chrome_args(Path::new("/tmp/prof"), OsString::from("--print-to-pdf=out.pdf"), "file:///a.html");
        assert!(args.iter().any(|a| a == "--disable-features=MacAppCodeSignClone"));
        assert!(args.iter().any(|a| a == "--user-data-dir=/tmp/prof"));
        assert_eq!(args.last().map(|a| a.as_os_str()), Some(std::ffi::OsStr::new("file:///a.html")));
    }

    #[test]
    fn temp_profile_is_removed_on_drop() {
        let profile = TempProfile::new();
        let dir = profile.0.clone();
        fs::create_dir_all(dir.join("Default")).unwrap();
        drop(profile);
        assert!(!dir.exists());
    }

    #[test]
    fn post_exit_wait_accepts_pdf_written_and_settled_after_launcher_exits() {
        let dir = TestDir::new("exit-test");
        let pdf = dir.0.join("chrome.pdf");

        let pdf_writer = pdf.clone();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            // Partial write without `%%EOF` while child processes are still rendering.
            let partial = vec![b'%'; 1500];
            fs::write(&pdf_writer, &partial).unwrap();

            thread::sleep(Duration::from_millis(20));
            let mut complete = partial;
            complete.extend_from_slice(&[b' '; 512]);
            complete.extend_from_slice(b"\n%%EOF\n");
            fs::write(&pdf_writer, &complete).unwrap();
            complete.len() as u64
        });

        assert!(wait_for_settled_pdf(&pdf, Duration::from_secs(5), Duration::from_millis(10), 5));
        let final_len = fs::metadata(&pdf).unwrap().len();
        let expected_len = writer.join().unwrap();
        assert_eq!(final_len, expected_len);
    }

    #[test]
    fn poll_settled_pdf_resets_quiet_count_when_incremental_eof_grows() {
        let dir = TestDir::new("increment-test");
        let pdf = dir.0.join("chrome.pdf");

        let mut last_len = None;
        let mut quiet_polls = 0;

        // Missing file never settles.
        assert!(!poll_settled_pdf(&pdf, &mut last_len, &mut quiet_polls, 5));
        assert_eq!(quiet_polls, 0);

        // First Skia increment already ends with `%%EOF`.
        let mut first_increment = vec![b'%'; 1500];
        first_increment.extend_from_slice(b"\n%%EOF\n");
        fs::write(&pdf, &first_increment).unwrap();

        // First observation records size; next 4 polls increment `quiet_polls` to 4 (< 5).
        assert!(!poll_settled_pdf(&pdf, &mut last_len, &mut quiet_polls, 5));
        assert_eq!(quiet_polls, 0);
        for expected_quiet in 1..=4 {
            assert!(!poll_settled_pdf(&pdf, &mut last_len, &mut quiet_polls, 5));
            assert_eq!(quiet_polls, expected_quiet);
        }

        // Second Skia increment grows the file before the 5th quiet poll: counter resets to 0.
        let mut final_increment = first_increment;
        final_increment.extend_from_slice(&[b' '; 1024]);
        final_increment.extend_from_slice(b"\n%%EOF\n");
        fs::write(&pdf, &final_increment).unwrap();

        assert!(!poll_settled_pdf(&pdf, &mut last_len, &mut quiet_polls, 5));
        assert_eq!(quiet_polls, 0);

        // Only after 5 consecutive unchanging polls at the new size does it report settled.
        for expected_quiet in 1..=4 {
            assert!(!poll_settled_pdf(&pdf, &mut last_len, &mut quiet_polls, 5));
            assert_eq!(quiet_polls, expected_quiet);
        }
        assert!(poll_settled_pdf(&pdf, &mut last_len, &mut quiet_polls, 5));
        assert_eq!(quiet_polls, 5);
    }

    #[test]
    fn post_exit_wait_times_out_when_pdf_is_never_written() {
        let dir = TestDir::new("timeout-test");
        let pdf = dir.0.join("missing.pdf");

        assert!(!wait_for_settled_pdf(&pdf, Duration::from_millis(50), Duration::from_millis(10), 5));
    }
}
