//! Microsoft Office document conversion (Word, Excel, PowerPoint) to PDF.
//!
//! Provides a headless converter abstraction with prioritized fallbacks:
//! - Windows: Native MS Office COM automation (PowerShell) -> LibreOffice (`soffice.exe` / `soffice`)
//! - Linux/macOS: LibreOffice (`soffice` / `libreoffice`) headless binary
//! - Wasm32: returns `ConversionError::NoConverterFound` (unsupported in browser)
//!
//! Converted PDFs are isolated in temporary directories and automatically cleaned up via RAII.

use std::path::{Path, PathBuf};

/// Supported Microsoft Office file extensions.
pub const OFFICE_EXTENSIONS: [&str; 6] = ["docx", "doc", "xlsx", "xls", "pptx", "ppt"];

/// Whether a given file extension is a supported Office format.
pub fn is_office_extension(ext: &str) -> bool {
    OFFICE_EXTENSIONS.iter().any(|&e| ext.eq_ignore_ascii_case(e))
}

/// Whether a file name indicates a temporary Microsoft Office lock file (starts with `~$`).
pub fn is_office_lock_file(name: &str) -> bool {
    let file_name = Path::new(name).file_name().and_then(|n| n.to_str()).unwrap_or(name);
    file_name.starts_with("~$")
}

/// Whether a file name has a supported Office extension and is not a lock file.
pub fn is_office_name(name: &str) -> bool {
    !is_office_lock_file(name) && name.rsplit_once('.').is_some_and(|(_, ext)| is_office_extension(ext))
}

/// Whether a path has a supported Office extension and is not a lock file.
pub fn is_office_path(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(is_office_name)
}

/// Classification of an Office document to the appropriate application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeApp {
    Word,
    Excel,
    PowerPoint,
}

impl OfficeApp {
    /// Classify from file extension.
    pub fn for_extension(ext: &str) -> Option<Self> {
        if ext.eq_ignore_ascii_case("docx") || ext.eq_ignore_ascii_case("doc") {
            Some(Self::Word)
        } else if ext.eq_ignore_ascii_case("xlsx") || ext.eq_ignore_ascii_case("xls") {
            Some(Self::Excel)
        } else if ext.eq_ignore_ascii_case("pptx") || ext.eq_ignore_ascii_case("ppt") {
            Some(Self::PowerPoint)
        } else {
            None
        }
    }

    /// Classify from file path.
    pub fn for_path(path: &Path) -> Option<Self> {
        path.extension().and_then(|e| e.to_str()).and_then(Self::for_extension)
    }
}

/// Conversion error type.
#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum ConversionError {
    #[error("unsupported file format: {0}")]
    UnsupportedFormat(String),
    #[error("file not found: {0}")]
    FileNotFound(String),
    #[error("no converter found: please install Microsoft Office or LibreOffice (soffice) to convert {0}")]
    NoConverterFound(String),
    #[error("conversion failed: {0}")]
    ProcessFailed(String),
    #[error("io error: {0}")]
    Io(String),
}

impl From<std::io::Error> for ConversionError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// RAII wrapper for a converted PDF file.
/// Holding this struct keeps the temporary directory alive.
/// When dropped, the temporary directory and all files inside are deleted.
#[derive(Debug)]
pub struct TempConvertedPdf {
    path: PathBuf,
    #[cfg(not(target_arch = "wasm32"))]
    _temp_dir: tempfile::TempDir,
}

impl TempConvertedPdf {
    /// Wrap a converted PDF path and its parent temporary directory.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(path: PathBuf, temp_dir: tempfile::TempDir) -> Self {
        Self { path, _temp_dir: temp_dir }
    }

    /// The path to the converted PDF file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the converted PDF bytes from disk.
    pub fn read_bytes(&self) -> Result<Vec<u8>, ConversionError> {
        std::fs::read(&self.path).map_err(|e| ConversionError::Io(e.to_string()))
    }
}

impl std::ops::Deref for TempConvertedPdf {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

/// Convert an Office document file on disk to a target PDF path.
pub fn convert_to_pdf(input_path: &Path, output_path: &Path) -> Result<PathBuf, ConversionError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (input_path, output_path);
        Err(ConversionError::NoConverterFound("Office document conversion is not supported in the web browser".into()))
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        if !input_path.is_file() {
            return Err(ConversionError::FileNotFound(input_path.display().to_string()));
        }

        #[cfg(target_os = "windows")]
        let app = OfficeApp::for_path(input_path).ok_or_else(|| ConversionError::UnsupportedFormat(input_path.display().to_string()))?;

        #[cfg(not(target_os = "windows"))]
        if OfficeApp::for_path(input_path).is_none() {
            return Err(ConversionError::UnsupportedFormat(input_path.display().to_string()));
        }

        // Ensure parent directory of output_path exists
        if let Some(parent) = output_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        #[cfg(target_os = "windows")]
        {
            // 1. Try native MS Office COM automation via PowerShell
            match convert_with_ms_office(app, input_path, output_path) {
                Ok(()) if output_path.is_file() => return Ok(output_path.to_path_buf()),
                Ok(()) | Err(_) => {
                    // Fall back to LibreOffice if MS Office conversion failed or produced no output
                }
            }

            // 2. Fall back to LibreOffice (soffice.exe / soffice)
            if let Some(soffice) = find_libreoffice() {
                convert_with_libreoffice(&soffice, input_path, output_path)?;
                return Ok(output_path.to_path_buf());
            }

            let name = input_path.file_name().and_then(|n| n.to_str()).unwrap_or("document");
            Err(ConversionError::NoConverterFound(name.to_string()))
        }

        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            if let Some(soffice) = find_libreoffice() {
                convert_with_libreoffice(&soffice, input_path, output_path)?;
                return Ok(output_path.to_path_buf());
            }

            let name = input_path.file_name().and_then(|n| n.to_str()).unwrap_or("document");
            Err(ConversionError::NoConverterFound(name.to_string()))
        }

        #[cfg(not(any(target_os = "windows", all(unix, not(target_arch = "wasm32")), target_arch = "wasm32")))]
        {
            let _ = app;
            Err(ConversionError::NoConverterFound(input_path.display().to_string()))
        }
    }
}

/// Convert an Office document file on disk into an isolated temporary PDF with RAII cleanup.
pub fn convert_to_temp_pdf(input_path: &Path) -> Result<TempConvertedPdf, ConversionError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = input_path;
        Err(ConversionError::NoConverterFound("Office document conversion is not supported in the web browser".into()))
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let temp_dir = tempfile::Builder::new().prefix("pdfcraft-office-").tempdir().map_err(|e| ConversionError::Io(e.to_string()))?;

        let output_path = temp_dir.path().join("converted.pdf");
        convert_to_pdf(input_path, &output_path)?;
        Ok(TempConvertedPdf::new(output_path, temp_dir))
    }
}

/// Convert an Office document from in-memory bytes into an isolated temporary PDF with RAII cleanup.
pub fn convert_bytes_to_pdf(name: &str, bytes: &[u8]) -> Result<TempConvertedPdf, ConversionError> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (name, bytes);
        Err(ConversionError::NoConverterFound("Office document conversion is not supported in the web browser".into()))
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        if !is_office_name(name) {
            return Err(ConversionError::UnsupportedFormat(name.to_string()));
        }

        let file_name = Path::new(name).file_name().and_then(|n| n.to_str()).unwrap_or(name);
        if file_name.is_empty() || !is_office_name(file_name) {
            return Err(ConversionError::UnsupportedFormat(name.to_string()));
        }

        let temp_dir = tempfile::Builder::new().prefix("pdfcraft-office-").tempdir().map_err(|e| ConversionError::Io(e.to_string()))?;

        let input_path = temp_dir.path().join(file_name);
        std::fs::write(&input_path, bytes).map_err(|e| ConversionError::Io(e.to_string()))?;

        let output_path = temp_dir.path().join("converted.pdf");
        convert_to_pdf(&input_path, &output_path)?;
        Ok(TempConvertedPdf::new(output_path, temp_dir))
    }
}

/// Escape single quotes and Unicode apostrophes for single-quoted PowerShell strings.
#[cfg(any(target_os = "windows", test))]
fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        out.push(c);
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Run a child process with a timeout, consuming stdout and stderr concurrently.
#[cfg(not(target_arch = "wasm32"))]
fn run_with_timeout(mut child: std::process::Child, timeout: std::time::Duration) -> Result<std::process::Output, ConversionError> {
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();
    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut h) = stdout_handle {
            let _ = std::io::Read::read_to_end(&mut h, &mut buf);
        }
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut h) = stderr_handle {
            let _ = std::io::Read::read_to_end(&mut h, &mut buf);
        }
        buf
    });

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_thread.join().unwrap_or_default();
                let stderr = stderr_thread.join().unwrap_or_default();
                return Ok(std::process::Output { status, stdout, stderr });
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                    return Err(ConversionError::ProcessFailed("process timed out".into()));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(ConversionError::ProcessFailed(format!("failed waiting for process: {e}"))),
        }
    }
}

/// Convert an Office document using native Microsoft Office COM automation via PowerShell.
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
fn convert_with_ms_office(app: OfficeApp, input_path: &Path, output_path: &Path) -> Result<(), ConversionError> {
    let in_str = input_path.to_str().ok_or_else(|| ConversionError::ProcessFailed("invalid input path".into()))?;
    let out_str = output_path.to_str().ok_or_else(|| ConversionError::ProcessFailed("invalid output path".into()))?;

    let in_quoted = ps_quote(in_str);
    let out_quoted = ps_quote(out_str);

    let script = match app {
        OfficeApp::Word => format!(
            r#"$ErrorActionPreference = 'Stop'
$in = {in_quoted}
$out = {out_quoted}
$word = $null
$doc = $null
try {{
    $word = New-Object -ComObject Word.Application
    $word.Visible = $false
    $word.DisplayAlerts = 0
    $word.AutomationSecurity = 3
    $doc = $word.Documents.Open($in)
    $doc.ExportAsFixedFormat($out, 17)
}} finally {{
    if ($doc -ne $null) {{
        $doc.Close(0)
        [System.Runtime.InteropServices.Marshal]::ReleaseComObject($doc) | Out-Null
    }}
    if ($word -ne $null) {{
        $word.Quit()
        [System.Runtime.InteropServices.Marshal]::ReleaseComObject($word) | Out-Null
    }}
    [System.GC]::Collect()
    [System.GC]::WaitForPendingFinalizers()
}}"#
        ),
        OfficeApp::Excel => format!(
            r#"$ErrorActionPreference = 'Stop'
$in = {in_quoted}
$out = {out_quoted}
$excel = $null
$wb = $null
try {{
    $excel = New-Object -ComObject Excel.Application
    $excel.Visible = $false
    $excel.DisplayAlerts = $false
    $excel.AutomationSecurity = 3
    $wb = $excel.Workbooks.Open($in)
    $wb.ExportAsFixedFormat(0, $out)
}} finally {{
    if ($wb -ne $null) {{
        $wb.Close($false)
        [System.Runtime.InteropServices.Marshal]::ReleaseComObject($wb) | Out-Null
    }}
    if ($excel -ne $null) {{
        $excel.Quit()
        [System.Runtime.InteropServices.Marshal]::ReleaseComObject($excel) | Out-Null
    }}
    [System.GC]::Collect()
    [System.GC]::WaitForPendingFinalizers()
}}"#
        ),
        OfficeApp::PowerPoint => format!(
            r#"$ErrorActionPreference = 'Stop'
$in = {in_quoted}
$out = {out_quoted}
$ppt = $null
$pres = $null
try {{
    $ppt = New-Object -ComObject PowerPoint.Application
    $ppt.AutomationSecurity = 3
    $pres = $ppt.Presentations.Open($in, -1, 0, 0)
    $pres.SaveAs($out, 32)
}} finally {{
    if ($pres -ne $null) {{
        $pres.Close()
        [System.Runtime.InteropServices.Marshal]::ReleaseComObject($pres) | Out-Null
    }}
    if ($ppt -ne $null) {{
        $ppt.Quit()
        [System.Runtime.InteropServices.Marshal]::ReleaseComObject($ppt) | Out-Null
    }}
    [System.GC]::Collect()
    [System.GC]::WaitForPendingFinalizers()
}}"#
        ),
    };

    let child = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ConversionError::ProcessFailed(format!("failed to spawn powershell: {e}")))?;

    let output = run_with_timeout(child, std::time::Duration::from_secs(60))?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        let out_msg = String::from_utf8_lossy(&output.stdout);
        let detail = if !err_msg.trim().is_empty() { err_msg.trim().to_string() } else { out_msg.trim().to_string() };
        return Err(ConversionError::ProcessFailed(format!("MS Office conversion failed: {detail}")));
    }

    Ok(())
}

/// Discover the LibreOffice (`soffice.exe` / `soffice`) binary on the system.
#[cfg(not(target_arch = "wasm32"))]
fn find_libreoffice() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        // 1. Check known default install directories
        let candidates = [r"C:\Program Files\LibreOffice\program\soffice.exe", r"C:\Program Files (x86)\LibreOffice\program\soffice.exe"];
        for path_str in candidates {
            let p = PathBuf::from(path_str);
            if p.is_file() {
                return Some(p);
            }
        }
        // Check %LOCALAPPDATA%\Programs\LibreOffice\program\soffice.exe
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let p = PathBuf::from(local_app_data).join(r"Programs\LibreOffice\program\soffice.exe");
            if p.is_file() {
                return Some(p);
            }
        }

        // 2. Check PATH with where.exe
        if let Ok(child) = std::process::Command::new("where.exe")
            .arg("soffice.exe")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            && let Ok(output) = run_with_timeout(child, std::time::Duration::from_secs(60))
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(first_line) = text.lines().next().map(str::trim).filter(|s| !s.is_empty()) {
                let p = PathBuf::from(first_line);
                if p.is_file() {
                    return Some(p);
                }
            }
        }

        // Test running directly
        if std::process::Command::new("soffice.exe")
            .arg("--version")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()
            .and_then(|child| run_with_timeout(child, std::time::Duration::from_secs(60)).ok())
            .is_some_and(|o| o.status.success())
        {
            return Some(PathBuf::from("soffice.exe"));
        }
        if std::process::Command::new("soffice")
            .arg("--version")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()
            .and_then(|child| run_with_timeout(child, std::time::Duration::from_secs(60)).ok())
            .is_some_and(|o| o.status.success())
        {
            return Some(PathBuf::from("soffice"));
        }
        None
    }

    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        // Check standard unix locations
        let candidates = [
            "/usr/bin/soffice",
            "/usr/bin/libreoffice",
            "/usr/local/bin/soffice",
            "/usr/local/bin/libreoffice",
            "/Applications/LibreOffice.app/Contents/MacOS/soffice",
        ];
        for path_str in candidates {
            let p = PathBuf::from(path_str);
            if p.is_file() {
                return Some(p);
            }
        }

        // Check PATH
        for cmd in ["soffice", "libreoffice"] {
            if std::process::Command::new(cmd)
                .arg("--version")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .ok()
                .and_then(|child| run_with_timeout(child, std::time::Duration::from_secs(60)).ok())
                .is_some_and(|o| o.status.success())
            {
                return Some(PathBuf::from(cmd));
            }
        }
        None
    }

    #[cfg(not(any(target_os = "windows", all(unix, not(target_arch = "wasm32")))))]
    {
        None
    }
}

/// Convert an Office document file using headless LibreOffice in an isolated user profile.
#[cfg(not(target_arch = "wasm32"))]
fn convert_with_libreoffice(soffice: &Path, input_path: &Path, output_path: &Path) -> Result<(), ConversionError> {
    let out_dir = output_path.parent().unwrap_or_else(|| Path::new("."));

    // Give LibreOffice its own isolated user profile so it doesn't conflict with any running instance.
    let profile_temp = tempfile::Builder::new().prefix("pdfcraft-lo-profile-").tempdir().map_err(|e| ConversionError::Io(e.to_string()))?;
    let profile_path = profile_temp.path().display().to_string().replace('\\', "/");
    let profile_uri = if cfg!(windows) { format!("file:///{profile_path}") } else { format!("file://{profile_path}") };

    let child = std::process::Command::new(soffice)
        .arg(format!("-env:UserInstallation={profile_uri}"))
        .args(["--headless", "--convert-to", "pdf", "--outdir"])
        .arg(out_dir)
        .arg(input_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ConversionError::ProcessFailed(format!("failed to spawn libreoffice: {e}")))?;

    let output = run_with_timeout(child, std::time::Duration::from_secs(60))?;

    if !output.status.success() {
        return Err(ConversionError::ProcessFailed(format!("LibreOffice exited with code {:?}", output.status.code())));
    }

    // LibreOffice names the output file <stem>.pdf in out_dir.
    let input_stem = input_path.file_stem().and_then(|s| s.to_str()).unwrap_or("document");
    let generated_pdf = out_dir.join(format!("{input_stem}.pdf"));

    if generated_pdf.is_file() && generated_pdf != output_path {
        // Rename or copy to desired output_path
        if output_path.exists() {
            let _ = std::fs::remove_file(output_path);
        }
        std::fs::rename(&generated_pdf, output_path)
            .or_else(|_| std::fs::copy(&generated_pdf, output_path).map(|_| ()))
            .map_err(|e| ConversionError::Io(e.to_string()))?;
        let _ = std::fs::remove_file(&generated_pdf);
    }

    if !output_path.is_file() {
        return Err(ConversionError::ProcessFailed("LibreOffice did not produce the expected PDF file".into()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_office_extensions() {
        assert!(is_office_extension("docx"));
        assert!(is_office_extension("DOCX"));
        assert!(is_office_extension("doc"));
        assert!(is_office_extension("xlsx"));
        assert!(is_office_extension("XLS"));
        assert!(is_office_extension("pptx"));
        assert!(is_office_extension("ppt"));
        assert!(!is_office_extension("pdf"));
        assert!(!is_office_extension("txt"));
        assert!(!is_office_extension("png"));
    }

    #[test]
    fn detects_office_names_and_paths() {
        assert!(is_office_name("report.docx"));
        assert!(is_office_name("budget.XLSX"));
        assert!(is_office_name("slides.pptx"));
        assert!(!is_office_name("document.pdf"));
        assert!(!is_office_name("image.png"));
        assert!(!is_office_name("plain.txt"));

        assert!(is_office_path(Path::new("C:/docs/file.docx")));
        assert!(is_office_path(Path::new("/home/user/sheet.xlsx")));
        assert!(!is_office_path(Path::new("/home/user/document.pdf")));
    }

    #[test]
    fn rejects_office_lock_files() {
        assert!(is_office_lock_file("~$report.docx"));
        assert!(is_office_lock_file("C:/docs/~$budget.xlsx"));
        assert!(!is_office_lock_file("report.docx"));
        assert!(!is_office_name("~$report.docx"));
        assert!(!is_office_path(Path::new("C:/docs/~$budget.xlsx")));
    }

    #[test]
    fn classifies_office_apps() {
        assert_eq!(OfficeApp::for_extension("docx"), Some(OfficeApp::Word));
        assert_eq!(OfficeApp::for_extension("doc"), Some(OfficeApp::Word));
        assert_eq!(OfficeApp::for_extension("xlsx"), Some(OfficeApp::Excel));
        assert_eq!(OfficeApp::for_extension("xls"), Some(OfficeApp::Excel));
        assert_eq!(OfficeApp::for_extension("pptx"), Some(OfficeApp::PowerPoint));
        assert_eq!(OfficeApp::for_extension("ppt"), Some(OfficeApp::PowerPoint));
        assert_eq!(OfficeApp::for_extension("pdf"), None);
    }

    #[test]
    fn ps_quote_escapes_single_quotes() {
        assert_eq!(ps_quote("hello"), "'hello'");
        assert_eq!(ps_quote("it's a test"), "'it''s a test'");
        assert_eq!(ps_quote("C:\\path with spaces\\doc.docx"), "'C:\\path with spaces\\doc.docx'");
    }

    #[test]
    fn ps_quote_escapes_smart_quotes() {
        assert_eq!(ps_quote("Bob\u{2018}s file"), "'Bob\u{2018}\u{2018}s file'");
        assert_eq!(ps_quote("Bob\u{2019}s file"), "'Bob\u{2019}\u{2019}s file'");
        assert_eq!(ps_quote("file\u{201A}test\u{201B}"), "'file\u{201A}\u{201A}test\u{201B}\u{201B}'");
    }

    #[test]
    fn nonexistent_file_returns_error() {
        let path = Path::new("nonexistent_test_file.docx");
        let result = convert_to_temp_pdf(path);
        assert!(matches!(result, Err(ConversionError::FileNotFound(_))));
    }

    #[test]
    fn unsupported_extension_returns_error() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_unsupported.xyz");
        let _ = std::fs::write(&path, b"dummy data");
        let result = convert_to_temp_pdf(&path);
        let _ = std::fs::remove_file(&path);
        assert!(matches!(result, Err(ConversionError::UnsupportedFormat(_))));
    }

    #[test]
    #[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
    fn test_live_office_conversion_if_installed() {
        let temp_dir = match tempfile::tempdir() {
            Ok(d) => d,
            Err(_) => return,
        };
        let docx_path = temp_dir.path().join("sample.docx");

        let ps_init = format!(
            r#"$word = $null; try {{ $word = New-Object -ComObject Word.Application; $doc = $word.Documents.Add(); $doc.Content.Text = "PdfCraft Test"; $doc.SaveAs2('{}'); $doc.Close(0); }} catch {{ exit 1 }} finally {{ if ($word) {{ $word.Quit() }} }}"#,
            docx_path.display()
        );
        let out = std::process::Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &ps_init]).output();

        if let Ok(res) = out
            && res.status.success()
            && docx_path.is_file()
        {
            let converted = convert_to_temp_pdf(&docx_path);
            assert!(converted.is_ok(), "Conversion should succeed: {:?}", converted.err());
            let pdf = converted.unwrap();
            assert!(pdf.path().is_file());
            let bytes = pdf.read_bytes().unwrap();
            assert!(bytes.starts_with(b"%PDF-"));
        }
    }
}
