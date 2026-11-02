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
            if is_complete(pdf) {
                return Ok(());
            }
            bail!("Chrome exited with {status} without writing {}; see {}", pdf.display(), log.display());
        }
        let len = fs::metadata(pdf).map(|m| m.len()).ok();
        if len == last_len && len.is_some() && is_complete(pdf) {
            quiet_polls += 1;
            if quiet_polls >= 10 {
                stop(&mut child);
                return Ok(());
            }
        } else {
            quiet_polls = 0;
        }
        last_len = len;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
