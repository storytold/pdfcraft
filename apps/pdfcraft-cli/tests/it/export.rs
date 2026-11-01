use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pdfcraft-cli-export-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn fixture(dir: &TempDirGuard) -> PathBuf {
    let pdf = dir.file("in.pdf");
    let bytes = pdfcraft_engine::Session::new().create_from_text("Export test", "Hello from PdfCraft").unwrap();
    std::fs::write(&pdf, bytes.as_slice()).unwrap();
    pdf
}

#[test]
fn export_writes_a_docx_package() {
    let dir = TempDirGuard::new("docx");
    let pdf = fixture(&dir);
    let out = dir.file("report.docx");
    let r = Command::new(BIN).args(["export", pdf.to_str().unwrap(), "--out", out.to_str().unwrap()]).output().unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(b"PK\x03\x04"), "not a DOCX package: {:?}", &bytes[..16]);
    assert!(bytes.windows(b"word/document.xml".len()).any(|w| w == b"word/document.xml"));
}

#[test]
fn export_refuses_to_replace_an_existing_file_without_force() {
    let dir = TempDirGuard::new("existing-without-force");
    let pdf = fixture(&dir);
    let out = dir.file("report.docx");
    std::fs::write(&out, b"keep this file").unwrap();

    let result = Command::new(BIN).args(["export", pdf.to_str().unwrap(), "--out", out.to_str().unwrap()]).output().unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("already exists"));
    assert_eq!(std::fs::read(out).unwrap(), b"keep this file");
}

#[test]
fn export_does_not_treat_a_password_as_force() {
    let dir = TempDirGuard::new("password-force-option");
    let pdf = fixture(&dir);
    let out = dir.file("report.docx");
    std::fs::write(&out, b"keep this file").unwrap();

    let result = Command::new(BIN).args(["export", pdf.to_str().unwrap(), "--password", "--force", "--out", out.to_str().unwrap()]).output().unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("already exists"));
    assert_eq!(std::fs::read(out).unwrap(), b"keep this file");
}

#[test]
fn export_force_replaces_an_existing_file() {
    let dir = TempDirGuard::new("force-existing");
    let pdf = fixture(&dir);
    let out = dir.file("report.docx");
    std::fs::write(&out, b"replace this file").unwrap();

    let result = Command::new(BIN).args(["export", pdf.to_str().unwrap(), "--out", out.to_str().unwrap(), "--force"]).output().unwrap();

    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(std::fs::read(out).unwrap().starts_with(b"PK\x03\x04"));
}

#[test]
fn export_force_refuses_a_symlink_destination() {
    let dir = TempDirGuard::new("force-symlink");
    let pdf = fixture(&dir);
    let target = dir.file("target.docx");
    let out = dir.file("report.docx");
    std::fs::write(&target, b"keep the symlink target").unwrap();
    if create_file_symlink(&target, &out).is_err() {
        return;
    }

    let result = Command::new(BIN).args(["export", pdf.to_str().unwrap(), "--out", out.to_str().unwrap(), "--force"]).output().unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symbolic link"));
    assert!(std::fs::symlink_metadata(&out).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read(target).unwrap(), b"keep the symlink target");
}

#[cfg(unix)]
fn create_file_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_file_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[test]
fn export_accepts_a_password_that_is_an_option_name() {
    let dir = TempDirGuard::new("password-option-name");
    let pdf = fixture(&dir);
    let out = dir.file("report.docx");
    let r = Command::new(BIN).args(["export", pdf.to_str().unwrap(), "--password", "--out", "--out", out.to_str().unwrap()]).output().unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(b"PK\x03\x04"), "not a DOCX package: {:?}", &bytes[..16]);
    assert!(bytes.windows(b"word/document.xml".len()).any(|w| w == b"word/document.xml"));
}
