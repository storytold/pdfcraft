//! Office documents (Word, Excel, PowerPoint, OpenDocument, RTF) to PDF through an installed
//! LibreOffice, run as a separate process: the "LibreOffice sidecar" of
//! `docs/file-format-parity.md`. PdfCraft never parses these formats itself.
//!
//! Detection ([`is_office`]) is pure and works everywhere. Conversion ([`office_to_pdf`]) runs
//! only on desktop builds: each run gets its own temporary folder and a fresh LibreOffice
//! profile with macros and untrusted links turned off, and is stopped after [`TIMEOUT`].

use crate::CreateError;

/// File extensions of the Office documents converted through LibreOffice.
pub const OFFICE_EXTENSIONS: &[&str] = &[
    // Word and Writer
    "doc", "docx", "docm", "dot", "dotx", "dotm", "odt", "ott", "rtf", //
    // Excel and Calc
    "xls", "xlsx", "xlsm", "xlt", "xltx", "xltm", "ods", "ots", //
    // PowerPoint and Impress
    "ppt", "pptx", "pptm", "pot", "potx", "potm", "pps", "ppsx", "ppsm", "odp", "otp",
];

/// The OLE compound-file signature of Office 97–2003 files (`.doc`, `.xls`, `.ppt`).
const OLE: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// The longest a conversion may run before LibreOffice is stopped. A fresh profile costs a
/// few seconds on its own; large spreadsheets take longer.
pub const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The largest PDF read back from LibreOffice.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const MAX_PDF: u64 = 512 * 1024 * 1024;

/// `name`'s extension as it appears in [`OFFICE_EXTENSIONS`], if it is one.
fn office_extension(name: &str) -> Option<&'static str> {
    let (_, ext) = name.rsplit_once('.')?;
    OFFICE_EXTENSIONS.iter().copied().find(|e| ext.eq_ignore_ascii_case(e))
}

/// Whether `bytes` named `name` is an Office document: an Office extension, and the bytes start
/// like one (an OLE compound file, a ZIP package as OOXML and OpenDocument are, or RTF).
pub fn is_office(name: &str, bytes: &[u8]) -> bool {
    office_extension(name).is_some() && (bytes.starts_with(OLE) || bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"{\\rtf"))
}

/// Convert the Office document `bytes` named `name` to PDF bytes with LibreOffice.
#[cfg(target_arch = "wasm32")]
pub fn office_to_pdf(name: &str, _bytes: &[u8]) -> Result<Vec<u8>, CreateError> {
    Err(CreateError::Office(format!("{name}: Office documents open only in the desktop app, which converts them with LibreOffice")))
}

/// Convert the Office document `bytes` named `name` to PDF bytes with LibreOffice.
#[cfg(not(target_arch = "wasm32"))]
pub fn office_to_pdf(name: &str, bytes: &[u8]) -> Result<Vec<u8>, CreateError> {
    let program = native::libreoffice().ok_or_else(|| {
        CreateError::Office(format!(
            "{name}: opening Word, Excel and PowerPoint files needs LibreOffice, which isn't installed (get it free from libreoffice.org)"
        ))
    })?;
    native::convert_with(&program, name, bytes, TIMEOUT)
}

/// Where LibreOffice is installed, if it is: `PDFCRAFT_SOFFICE`, then the `PATH`, then the
/// usual install folders.
#[cfg(not(target_arch = "wasm32"))]
pub fn libreoffice() -> Option<std::path::PathBuf> {
    native::libreoffice()
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{CreateError, MAX_PDF, office_extension};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    /// The program names LibreOffice installs on the `PATH`.
    #[cfg(windows)]
    const PATH_NAMES: &[&str] = &["soffice.exe"];
    #[cfg(not(windows))]
    const PATH_NAMES: &[&str] = &["soffice", "libreoffice"];

    pub(super) fn libreoffice() -> Option<PathBuf> {
        let defaults = default_locations(&|key| std::env::var_os(key));
        candidates(std::env::var_os("PDFCRAFT_SOFFICE"), std::env::var_os("PATH"), defaults).into_iter().find(|p| p.is_file())
    }

    /// Where to look, in order: the override, each `PATH` folder, then `defaults`.
    pub(super) fn candidates(override_path: Option<OsString>, path: Option<OsString>, defaults: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = override_path.filter(|p| !p.is_empty()).map(PathBuf::from).into_iter().collect();
        if let Some(path) = path {
            for dir in std::env::split_paths(&path) {
                out.extend(PATH_NAMES.iter().map(|name| dir.join(name)));
            }
        }
        out.extend(defaults);
        out
    }

    /// The usual install locations, given a way to read environment variables.
    #[cfg(windows)]
    pub(super) fn default_locations(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
        ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
            .iter()
            .filter_map(|key| env(key))
            .map(|dir| PathBuf::from(dir).join("LibreOffice").join("program").join("soffice.exe"))
            .collect()
    }

    /// The usual install locations, given a way to read environment variables.
    #[cfg(target_os = "macos")]
    pub(super) fn default_locations(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
        let app = Path::new("LibreOffice.app/Contents/MacOS/soffice");
        let mut out = vec![Path::new("/Applications").join(app)];
        out.extend(env("HOME").map(|home| PathBuf::from(home).join("Applications").join(app)));
        out
    }

    /// The usual install locations, given a way to read environment variables.
    #[cfg(not(any(windows, target_os = "macos")))]
    pub(super) fn default_locations(_env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
        [
            "/usr/bin/soffice",
            "/usr/bin/libreoffice",
            "/usr/local/bin/soffice",
            "/usr/local/bin/libreoffice",
            "/usr/lib/libreoffice/program/soffice",
            "/opt/libreoffice/program/soffice",
            "/snap/bin/libreoffice",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    }

    /// The fresh profile's settings: no macros at all, and no links to untrusted locations.
    const PROFILE_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="BlockUntrustedRefererLinks" oor:op="fuse"><value>true</value></prop></item>
</oor:items>
"#;

    /// A private temporary folder, removed (with everything in it) when dropped.
    pub(super) struct WorkDir(pub(super) PathBuf);

    impl WorkDir {
        pub(super) fn new() -> std::io::Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
            let dir = std::env::temp_dir().join(format!("pdfcraft-office-{}-{n}-{nanos}", std::process::id()));
            // `create_dir`, not `create_dir_all`: a folder that already exists is not ours.
            std::fs::create_dir(&dir)?;
            Ok(Self(dir))
        }
    }

    impl Drop for WorkDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `path` as a `file://` URL, which `-env:UserInstallation` takes.
    pub(super) fn file_url(path: &Path) -> String {
        let path = path.to_string_lossy().replace('\\', "/");
        let mut url = String::from("file://");
        // A Windows path (`C:/…`) needs the third slash a Unix path already starts with.
        if !path.starts_with('/') {
            url.push('/');
        }
        for b in path.bytes() {
            if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
                url.push(char::from(b));
            } else {
                url.push_str(&format!("%{b:02X}"));
            }
        }
        url
    }

    pub(super) enum Run {
        Exited(ExitStatus),
        TimedOut,
    }

    /// Run `command` to completion, killing it once `timeout` passes.
    pub(super) fn run_with_timeout(mut command: Command, timeout: Duration) -> std::io::Result<Run> {
        let mut child = command.spawn()?;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(Run::Exited(status)),
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            }
            if start.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(Run::TimedOut);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Convert with the LibreOffice at `program`, giving up after `timeout`.
    pub(super) fn convert_with(program: &Path, name: &str, bytes: &[u8], timeout: Duration) -> Result<Vec<u8>, CreateError> {
        let failed = |why: String| CreateError::Office(format!("{name}: {why}"));
        let ext = office_extension(name).ok_or_else(|| failed("this isn't a Word, Excel or PowerPoint file".into()))?;
        let work = WorkDir::new().map_err(|e| failed(format!("couldn't make a temporary folder: {e}")))?;
        // A fixed input name: the document's own name never reaches LibreOffice's command line.
        let input = work.0.join(format!("document.{ext}"));
        let profile = work.0.join("profile");
        let out = work.0.join("out");
        let prepared = std::fs::write(&input, bytes)
            .and_then(|()| std::fs::create_dir_all(profile.join("user")))
            .and_then(|()| std::fs::write(profile.join("user").join("registrymodifications.xcu"), PROFILE_SETTINGS))
            .and_then(|()| std::fs::create_dir(&out));
        prepared.map_err(|e| failed(format!("couldn't prepare the conversion: {e}")))?;

        let mut command = Command::new(program);
        command
            .arg(format!("-env:UserInstallation={}", file_url(&profile)))
            .args(["--headless", "--invisible", "--nologo", "--norestore", "--nodefault", "--nolockcheck", "--convert-to", "pdf", "--outdir"])
            .arg(&out)
            .arg(&input)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window flashes up during the conversion.
            command.creation_flags(0x0800_0000);
        }
        let status = match run_with_timeout(command, timeout) {
            Ok(Run::Exited(status)) => status,
            Ok(Run::TimedOut) => return Err(failed(format!("LibreOffice took longer than {} seconds and was stopped", timeout.as_secs()))),
            Err(e) => return Err(failed(format!("couldn't start LibreOffice ({}): {e}", program.display()))),
        };
        let pdf = out.join("document.pdf");
        let size = match std::fs::metadata(&pdf) {
            Ok(meta) => meta.len(),
            Err(_) if !status.success() => return Err(failed(format!("LibreOffice couldn't convert it ({status})"))),
            Err(_) => return Err(failed("LibreOffice didn't produce a PDF; the file may be damaged or password-protected".into())),
        };
        if size > MAX_PDF {
            return Err(failed(format!("the converted PDF is larger than {} MB", MAX_PDF / (1024 * 1024))));
        }
        let bytes = std::fs::read(&pdf).map_err(|e| failed(format!("couldn't read the converted PDF: {e}")))?;
        if !bytes.starts_with(b"%PDF-") {
            return Err(failed("LibreOffice's output isn't a PDF".into()));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCX_HEAD: &[u8] = b"PK\x03\x04\x14\x00\x06\x00";

    #[test]
    fn office_files_are_recognised_by_extension_and_signature() {
        assert!(is_office("Report.docx", DOCX_HEAD));
        assert!(is_office("SHEET.XLSX", DOCX_HEAD));
        assert!(is_office("deck.odp", DOCX_HEAD));
        assert!(is_office("old.doc", OLE));
        assert!(is_office("old.ppt", &[OLE, b"rest"].concat()));
        assert!(is_office("letter.rtf", b"{\\rtf1\\ansi"));
        // Word saves RTF under .doc too.
        assert!(is_office("letter.doc", b"{\\rtf1\\ansi"));
    }

    #[test]
    fn other_files_are_not_office_documents() {
        // The right signature with another extension, or an Office name with other bytes.
        assert!(!is_office("archive.zip", DOCX_HEAD));
        assert!(!is_office("Report.docx", b"%PDF-1.7"));
        assert!(!is_office("Report.docx", b"plain text"));
        assert!(!is_office("Report.docx", b""));
        assert!(!is_office("docx", DOCX_HEAD));
        assert!(!is_office("", DOCX_HEAD));
        // A multi-byte name never splits a character.
        assert!(!is_office("résumé.dócx", DOCX_HEAD));
        assert!(is_office("résumé.docx", DOCX_HEAD));
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod native {
        use super::super::native::*;
        use super::DOCX_HEAD;
        use std::ffi::OsString;
        use std::path::{Path, PathBuf};
        use std::time::Duration;

        #[test]
        fn file_urls_escape_spaces_and_keep_drive_letters() {
            assert_eq!(file_url(Path::new("/tmp/a b/profile")), "file:///tmp/a%20b/profile");
            assert_eq!(file_url(Path::new(r"C:\Users\Jo Ann\profile")), "file:///C:/Users/Jo%20Ann/profile");
            assert_eq!(file_url(Path::new("/tmp/é")), "file:///tmp/%C3%A9");
        }

        #[test]
        fn the_override_comes_first_then_the_path_then_the_defaults() {
            let path = std::env::join_paths([Path::new("first"), Path::new("second")]).unwrap();
            let found = candidates(Some(OsString::from("custom/soffice")), Some(path), vec![PathBuf::from("default/soffice")]);
            assert_eq!(found.first(), Some(&PathBuf::from("custom/soffice")));
            assert!(found.get(1).is_some_and(|p| p.starts_with("first")));
            assert_eq!(found.last(), Some(&PathBuf::from("default/soffice")));
            // An empty override is no override.
            assert_eq!(candidates(Some(OsString::new()), None, Vec::new()), Vec::<PathBuf>::new());
        }

        #[test]
        fn default_locations_read_only_the_given_environment() {
            let found = default_locations(&|_| None);
            assert!(found.iter().all(|p| p.is_absolute()), "{found:?}");
        }

        #[test]
        fn a_missing_converter_is_an_error_not_a_crash() {
            let missing = std::env::temp_dir().join("pdfcraft-no-such-soffice").join("soffice");
            let error = convert_with(&missing, "Report.docx", DOCX_HEAD, Duration::from_secs(5)).unwrap_err();
            assert!(error.to_string().contains("couldn't start LibreOffice"), "{error}");
        }

        #[test]
        fn a_converter_that_makes_no_pdf_is_reported() {
            // This test binary stands in for LibreOffice: it rejects the arguments and exits
            // without writing anything.
            let program = std::env::current_exe().unwrap();
            let error = convert_with(&program, "Report.docx", DOCX_HEAD, Duration::from_secs(60)).unwrap_err();
            assert!(error.to_string().contains("LibreOffice"), "{error}");
        }

        #[test]
        fn a_converter_that_hangs_is_stopped() {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--ignored", "--exact", "office::tests::native::sleeps_for_a_minute", "--nocapture"]);
            command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            let started = std::time::Instant::now();
            assert!(matches!(run_with_timeout(command, Duration::from_millis(500)), Ok(Run::TimedOut)));
            assert!(started.elapsed() < Duration::from_secs(30));
        }

        /// The stand-in for a LibreOffice that never finishes (run only by the test above).
        #[test]
        #[ignore]
        fn sleeps_for_a_minute() {
            std::thread::sleep(Duration::from_secs(60));
        }

        #[test]
        fn temporary_folders_are_removed_with_their_contents() {
            let work = WorkDir::new().unwrap();
            let other = WorkDir::new().unwrap();
            assert_ne!(work.0, other.0);
            let dir = work.0.clone();
            std::fs::create_dir_all(dir.join("profile").join("user")).unwrap();
            std::fs::write(dir.join("document.docx"), DOCX_HEAD).unwrap();
            drop(work);
            assert!(!dir.exists());
        }

        /// A real conversion, when LibreOffice is installed (skipped otherwise).
        #[test]
        fn libreoffice_converts_a_document_when_installed() {
            let Some(program) = super::super::libreoffice() else {
                eprintln!("skipped: LibreOffice isn't installed");
                return;
            };
            let rtf = b"{\\rtf1\\ansi{\\fonttbl\\f0\\fswiss Helvetica;}\\f0\\pard Hello from PdfCraft.\\par}";
            let pdf = convert_with(&program, "hello.rtf", rtf, super::super::TIMEOUT).unwrap();
            assert!(pdf.starts_with(b"%PDF-"));
        }
    }
}
