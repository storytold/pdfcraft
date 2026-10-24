//! Word, Excel, PowerPoint and OpenDocument files to PDF.
//!
//! PdfCraft has no layout engine for these formats. On Windows, Create first tries Microsoft
//! Office via headless COM automation when installed, and otherwise hands the file to a bundled
//! portable or system LibreOffice in a private temporary folder.

#[cfg(target_arch = "wasm32")]
use crate::EditError;

/// Where to download LibreOffice, named in the "not installed" message.
pub const DOWNLOAD_URL: &str = "https://www.libreoffice.org/download/";

/// Set to the full path of `soffice` to use a LibreOffice that isn't on the PATH or in a
/// standard place.
pub const PROGRAM_ENV: &str = "PDFCRAFT_SOFFICE";

#[cfg(target_arch = "wasm32")]
pub fn find_libreoffice() -> Option<std::path::PathBuf> {
    None
}

#[cfg(target_arch = "wasm32")]
pub fn convert(name: &str, _bytes: &[u8]) -> Result<Vec<u8>, EditError> {
    Err(EditError::Source(format!("{name}: Office files can only be converted in the desktop app")))
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{convert, find_libreoffice};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::{DOWNLOAD_URL, PROGRAM_ENV};
    use crate::EditError;

    /// A conversion that takes longer than this is stopped (a hung or first-run converter).
    const TIMEOUT: Duration = Duration::from_secs(180);

    /// `soffice` (or `libreoffice`) from `PDFCRAFT_SOFFICE`, a portable folder alongside the
    /// executable, the PATH, or a standard install place.
    pub fn find_libreoffice() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os(PROGRAM_ENV).map(PathBuf::from).filter(|p| p.is_file()) {
            return Some(p);
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
                if portable.is_file() {
                    return Some(portable);
                }
            }
        }
        let names: &[&str] = if cfg!(windows) { &["soffice.exe", "soffice.com"] } else { &["soffice", "libreoffice"] };
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                if let Some(found) = names.iter().map(|n| dir.join(n)).find(|p| p.is_file()) {
                    return Some(found);
                }
            }
        }
        [
            "/Applications/LibreOffice.app/Contents/MacOS/soffice",
            r"C:\Program Files\LibreOffice\program\soffice.exe",
            r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
            "/usr/bin/soffice",
            "/usr/local/bin/soffice",
            "/opt/libreoffice/program/soffice",
            "/var/lib/flatpak/exports/bin/org.libreoffice.LibreOffice",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
    }

    /// A private working folder that is removed when it goes out of scope.
    struct WorkDir(PathBuf);

    impl WorkDir {
        fn new() -> std::io::Result<Self> {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let dir = std::env::temp_dir().join(format!("pdfcraft-office-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir)?;
            Ok(Self(dir))
        }
    }

    impl Drop for WorkDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A `file://` URL for LibreOffice's `-env:UserInstallation` (its own profile folder, so a
    /// conversion never fights with a LibreOffice the person has open).
    pub(super) fn file_url(path: &Path) -> String {
        let p = path.to_string_lossy().replace('\\', "/");
        let p = p.replace('%', "%25").replace(' ', "%20").replace('#', "%23");
        if p.starts_with('/') { format!("file://{p}") } else { format!("file:///{p}") }
    }

    #[cfg(windows)]
    fn try_convert_ms_office(ext: &str, input: &Path, out_dir: &Path) -> Option<Vec<u8>> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let output = out_dir.join("ms_output.pdf");
        let in_str = input.to_string_lossy().replace('\'', "''");
        let out_str = output.to_string_lossy().replace('\'', "''");

        let script = match ext {
            "doc" | "docx" | "dot" | "dotx" | "rtf" | "odt" | "ott" => format!(
                "$ErrorActionPreference = 'Stop'; $w = New-Object -ComObject Word.Application; try {{$w.Visible = $false; $w.DisplayAlerts = 0; $d =$w.Documents.Open('{in_str}', $false,$true, $false);$d.ExportAsFixedFormat('{out_str}', 17); $d.Close(0); }} finally {{$w.Quit(); }}"
            ),
            "xls" | "xlsx" | "ods" => format!(
                "$ErrorActionPreference = 'Stop'; $x = New-Object -ComObject Excel.Application; try {{$x.Visible = $false; $x.DisplayAlerts = $false; $b = $x.Workbooks.Open('{in_str}', 0,$true); $b.ExportAsFixedFormat(0, '{out_str}');$b.Close($false); }} finally {{$x.Quit(); }}"
            ),
            "ppt" | "pptx" | "odp" => format!(
                "$ErrorActionPreference = 'Stop'; $p = New-Object -ComObject PowerPoint.Application; try {{$d = $p.Presentations.Open('{in_str}', -1, 0, 0);$d.SaveAs('{out_str}', 32); $d.Close(); }} finally {{$p.Quit(); }}"
            ),
            _ => return None,
        };

        let mut child = Command::new("powershell")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if started.elapsed() > TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => return None,
            }
        };

        if !status.success() {
            return None;
        }
        let pdf = std::fs::read(&output).ok()?;
        pdf.starts_with(b"%PDF-").then_some(pdf)
    }

    /// Convert an Office file's bytes to PDF bytes. `name` decides the format (its extension) and
    /// names the file in error messages.
    pub fn convert(name: &str, bytes: &[u8]) -> Result<Vec<u8>, EditError> {
        let fail = |why: &str| EditError::Source(format!("{name}: {why}"));
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).filter(|e| pdfcraft_create::OFFICE.contains(&e.as_str()));
        let Some(ext) = ext else { return Err(fail("not an Office file PdfCraft can convert")) };

        let work = WorkDir::new().map_err(|e| fail(&format!("couldn't make a temporary folder ({e})")))?;
        let (input, out_dir, profile) = (work.0.join(format!("input.{ext}")), work.0.join("out"), work.0.join("profile"));
        std::fs::create_dir_all(&out_dir).map_err(|e| fail(&format!("couldn't make a temporary folder ({e})")))?;
        std::fs::write(&input, bytes).map_err(|e| fail(&format!("couldn't write a temporary copy ({e})")))?;

        #[cfg(windows)]
        if let Some(pdf) = try_convert_ms_office(&ext, &input, &out_dir) {
            return Ok(pdf);
        }

        let Some(program) = find_libreoffice() else {
            return Err(fail(&format!(
                "converting Word, Excel and PowerPoint files needs Microsoft Office or LibreOffice, which wasn't found. Install LibreOffice from {DOWNLOAD_URL} (or place LibreOfficePortable next to the app), then try again"
            )));
        };

        let mut child = Command::new(&program)
            .arg(format!("-env:UserInstallation={}", file_url(&profile)))
            .args(["--headless", "--norestore", "--nolockcheck", "--nodefault", "--nologo", "--convert-to", "pdf", "--outdir"])
            .arg(&out_dir)
            .arg(&input)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| fail(&format!("LibreOffice couldn't be started ({e})")))?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() > TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(fail("LibreOffice took too long and was stopped"));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => return Err(fail(&format!("LibreOffice stopped unexpectedly ({e})"))),
            }
        };
        let pdf = std::fs::read(out_dir.join("input.pdf")).map_err(|_| {
            if status.success() {
                fail("LibreOffice finished but produced no PDF (the file may be damaged or password-protected)")
            } else {
                fail("LibreOffice couldn't convert this file")
            }
        })?;
        if !pdf.starts_with(b"%PDF-") {
            return Err(fail("LibreOffice produced something that isn't a PDF"));
        }
        Ok(pdf)
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

    #[test]
    fn a_non_office_name_is_refused_before_anything_runs() {
        let res = convert("photo.jpg", b"x");
        assert!(res.is_err());
        let err = res.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("not an Office file"), "{err}");
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
        let url = super::native::file_url(std::path::Path::new("/tmp/my dir/100%#x"));
        assert_eq!(url, "file:///tmp/my%20dir/100%25%23x");
    }
}
