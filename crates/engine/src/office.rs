//! Word, Excel, PowerPoint and OpenDocument files to PDF.
//!
//! PdfCraft has no layout engine for these formats. Create hands the file to a bundled portable or
//! system LibreOffice (`soffice --headless --convert-to pdf`) in a private temporary folder, and
//! reads the PDF it writes back.

#[cfg(target_arch = "wasm32")]
use crate::EditError;

/// Where to download LibreOffice, named in the "not installed" message.
pub const DOWNLOAD_URL: &str = "https://www.libreoffice.org/download/";

/// Set to the full path of `soffice` to use a LibreOffice that isn't on the PATH or in a
/// standard place.
pub const PROGRAM_ENV: &str = "PDFCRAFT_SOFFICE";

/// Always returns `None` on WebAssembly builds because external converters cannot be spawned.
#[cfg(target_arch = "wasm32")]
pub fn find_libreoffice() -> Option<std::path::PathBuf> {
    None
}

/// Refuses Office conversion on WebAssembly builds where external processes are unavailable.
#[cfg(target_arch = "wasm32")]
pub fn convert(name: &str, _bytes: &[u8]) -> Result<Vec<u8>, EditError> {
    Err(EditError::Source(format!("{name}: Office files can only be converted in the desktop app")))
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{convert, find_libreoffice};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use super::{DOWNLOAD_URL, PROGRAM_ENV};
    use crate::EditError;

    /// A conversion that takes longer than this is stopped (a hung or first-run LibreOffice).
    const TIMEOUT: Duration = Duration::from_secs(180);

    /// Maximum size of a converted PDF read back into memory (256 MiB).
    const MAX_PDF_BYTES: u64 = 256 * 1024 * 1024;

    /// Monotonic counter used to disambiguate concurrent temporary working directories.
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[cfg(unix)]
    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        path.is_file() && (path.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false))
    }

    #[cfg(not(unix))]
    fn is_executable(path: &Path) -> bool {
        path.is_file()
    }

    /// Locate `soffice` (or `libreoffice`) from `PDFCRAFT_SOFFICE`, a portable folder alongside
    /// the executable, the `PATH`, or a standard OS installation path.
    pub fn find_libreoffice() -> Option<PathBuf> {
        if let Some(val) = std::env::var_os(PROGRAM_ENV) {
            let p = PathBuf::from(val);
            if p.is_file() && is_executable(&p) {
                return Some(p);
            }
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(exe_dir) = exe.parent()
        {
            for rel in [
                "libreoffice/program/soffice.exe",
                "libreoffice/App/libreoffice/program/soffice.exe",
                "LibreOfficePortable/App/libreoffice/program/soffice.exe",
                "LibreOfficePortable/program/soffice.exe",
                "libreoffice/program/soffice",
            ] {
                let portable = exe_dir.join(rel);
                if is_executable(&portable) {
                    return Some(portable);
                }
            }
        }
        let names: &[&str] = if cfg!(windows) { &["soffice.exe", "soffice.com"] } else { &["soffice", "libreoffice"] };
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                if let Some(found) = names.iter().map(|n| dir.join(n)).find(|p| is_executable(p)) {
                    return Some(found);
                }
            }
        }
        [
            PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS/soffice"),
            PathBuf::from(r"C:\Program Files\LibreOffice\program\soffice.exe"),
            PathBuf::from(r"C:\Program Files (x86)\LibreOffice\program\soffice.exe"),
            PathBuf::from("/usr/bin/soffice"),
            PathBuf::from("/usr/local/bin/soffice"),
            PathBuf::from("/opt/libreoffice/program/soffice"),
            PathBuf::from("/var/lib/flatpak/exports/bin/org.libreoffice.LibreOffice"),
        ]
        .into_iter()
        .find(|p| is_executable(p))
    }

    /// A newly created private working folder (mode `0700` on Unix) that is removed on drop.
    struct WorkDir(PathBuf);

    impl WorkDir {
        fn new() -> std::io::Result<Self> {
            let base = std::env::temp_dir();
            let pid = std::process::id();
            for attempt in 0..64u32 {
                let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
                let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
                let dir = base.join(format!("pdfcraft-office-{pid}-{nanos}-{seq}-{attempt}"));
                let res = {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::DirBuilderExt;
                        std::fs::DirBuilder::new().mode(0o700).create(&dir)
                    }
                    #[cfg(not(unix))]
                    {
                        std::fs::create_dir(&dir)
                    }
                };
                match res {
                    Ok(()) => return Ok(Self(dir)),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e),
                }
            }
            Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "could not allocate a unique temporary folder"))
        }
    }

    impl Drop for WorkDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Format a `file://` URL for LibreOffice's `-env:UserInstallation` flag.
    pub(super) fn file_url(path: &Path) -> String {
        let p = path.to_string_lossy();
        #[cfg(unix)]
        let p = p.replace('\', "%5C");
        #[cfg(not(unix))]
        let p = p.replace('\', "/");
        let p = p.replace('%', "%25")
                 .replace(' ', "%20")
                 .replace('#', "%23")
                 .replace('?', "%3F");
        if p.starts_with('/') { format!("file://{p}") } else { format!("file:///{p}") }
    }

    /// Terminate both the `soffice` launcher and any `soffice.bin` descendant processes.
    fn kill_process_tree(child: &mut Child) {
        let pid = child.id();
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    pub(super) fn read_bounded_pdf(path: &Path, max_bytes: u64) -> Result<Vec<u8>, &'static str> {
        let meta = std::fs::metadata(path).map_err(|_| "missing")?;
        if !meta.is_file() {
            return Err("missing");
        }
        if meta.len() > max_bytes {
            return Err("too_large");
        }
        std::fs::read(path).map_err(|_| "missing")
    }

    pub(super) fn convert_with(program: Option<&Path>, name: &str, bytes: &[u8]) -> Result<Vec<u8>, EditError> {
        let fail = |why: &str| EditError::Source(format!("{name}: {why}"));
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).filter(|e| pdfcraft_create::OFFICE.contains(&e.as_str()));
        let Some(ext) = ext else { return Err(fail("not an Office file PdfCraft can convert")) };

        if let Some(val) = std::env::var_os(PROGRAM_ENV) {
            let p = PathBuf::from(&val);
            if !p.is_file() || !is_executable(&p) {
                return Err(fail(&format!(
                    "converting Word, Excel and PowerPoint files needs LibreOffice, but PDFCRAFT_SOFFICE names a missing or non-executable file ({:?}). Install it from {DOWNLOAD_URL}",
                    val
                )));
            }
        }

        let Some(program) = program else {
            return Err(fail(&format!(
                "converting Word, Excel and PowerPoint files needs LibreOffice, which wasn't found. Install it from {DOWNLOAD_URL} (it is free), then try again"
            )));
        };

        let work = WorkDir::new().map_err(|e| fail(&format!("couldn't make a temporary folder ({e})")))?;
        let (input, out_dir, profile) = (work.0.join(format!("input.{ext}")), work.0.join("out"), work.0.join("profile"));
        std::fs::create_dir(&out_dir).map_err(|e| fail(&format!("couldn't make a temporary folder ({e})")))?;
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&input)
                .map_err(|e| fail(&format!("couldn't write a temporary copy ({e})")))?;
            file.write_all(bytes).map_err(|e| fail(&format!("couldn't write a temporary copy ({e})")))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&input, bytes).map_err(|e| fail(&format!("couldn't write a temporary copy ({e})")))?;
        }

        let mut cmd = Command::new(program);
        cmd.arg(format!("-env:UserInstallation={}", file_url(&profile)))
            .args(["--headless", "--norestore", "--nolockcheck", "--nodefault", "--nologo", "--convert-to", "pdf", "--outdir"])
            .arg(&out_dir)
            .arg(&input)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().map_err(|e| fail(&format!("LibreOffice couldn't be started ({e})")))?;
        let started = Instant::now();
        let out_pdf = out_dir.join("input.pdf");
        let status = loop {
            if let Ok(meta) = std::fs::metadata(&out_pdf)
                && meta.is_file()
                && meta.len() > MAX_PDF_BYTES
            {
                kill_process_tree(&mut child);
                return Err(fail("the converted PDF exceeds the maximum allowed size"));
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() > TIMEOUT => {
                    kill_process_tree(&mut child);
                    return Err(fail("LibreOffice took too long and was stopped"));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => {
                    kill_process_tree(&mut child);
                    return Err(fail(&format!("LibreOffice stopped unexpectedly ({e})")));
                }
            }
        };
        let pdf = match read_bounded_pdf(&out_pdf, MAX_PDF_BYTES) {
            Ok(bytes) => bytes,
            Err("too_large") => return Err(fail("the converted PDF exceeds the maximum allowed size")),
            Err(_) => {
                if status.success() {
                    return Err(fail("LibreOffice finished but produced no PDF (the file may be damaged or password-protected)"));
                }
                return Err(fail("LibreOffice couldn't convert this file"));
            }
        };
        if !pdf.starts_with(b"%PDF-") {
            return Err(fail("LibreOffice produced something that isn't a PDF"));
        }
        Ok(pdf)
    }

    pub fn convert(name: &str, bytes: &[u8]) -> Result<Vec<u8>, EditError> {
        let program = find_libreoffice();
        convert_with(program.as_deref(), name, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_extensions_are_detected_by_name() {
        use pdfcraft_create::{SourceKind, source_kind};
        assert_eq!(source_kind("Report.DOCX", b"PK\x03\x04"), Some(SourceKind::Office));
        assert_eq!(source_kind("budget.xlsx", b"PK\x03\x04"), Some(SourceKind::Office));
        assert_eq!(source_kind("deck.pptx", b"PK\x03\x04"), Some(SourceKind::Office));
        assert_eq!(source_kind("old.doc", b"\xD0\xCF\x11\xE0"), Some(SourceKind::Office));
        assert_eq!(source_kind("program.exe", b"MZ"), None);
        assert_eq!(source_kind("noextension", b"hello"), None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_non_office_name_is_refused_before_anything_runs() {
        let res = convert("photo.jpg", b"x");
        assert!(res.is_err());
        let err = res.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("not an Office file"), "{err}");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn missing_libreoffice_reports_download_url() {
        let err = super::native::convert_with(None, "report.docx", b"PK\x03\x04")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("needs LibreOffice, which wasn't found"), "{err}");
        assert!(err.contains(DOWNLOAD_URL), "{err}");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn read_bounded_pdf_rejects_oversized_outputs() {
        let path = std::env::temp_dir().join(format!("pdfcraft-size-test-{}.pdf", std::process::id()));
        std::fs::write(&path, b"%PDF-1.4 oversized").unwrap();
        assert_eq!(super::native::read_bounded_pdf(&path, 4), Err("too_large"));
        assert!(super::native::read_bounded_pdf(&path, 1024).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn converts_an_office_file_when_libreoffice_is_installed() {
        if find_libreoffice().is_none() {
            return;
        }
        let rtf = br"{\rtf1\ansi\deff0 Hello from PdfCraft\par}";
        let pdf = convert("greeting.rtf", rtf).unwrap_or_default();
        assert!(pdf.starts_with(b"%PDF-"), "expected a PDF, got {} bytes", pdf.len());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn file_urls_escape_what_would_break_them() {
        let url = super::native::file_url(std::path::Path::new("/tmp/my?dir/100%#x\bar"));
        assert_eq!(url, "file:///tmp/my%3Fdir/100%25%23x%5Cbar");
    }
}
