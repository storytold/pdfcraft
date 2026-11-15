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
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
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
    native::convert(&program, name, bytes)
}

/// Where LibreOffice is installed, if it is: `PDFCRAFT_SOFFICE`, then the `PATH`, then the
/// usual install folders.
#[cfg(not(target_arch = "wasm32"))]
pub fn libreoffice() -> Option<std::path::PathBuf> {
    native::libreoffice()
}

/// The tool that keeps LibreOffice off the network while it converts (`bwrap`, `unshare` or
/// `sandbox-exec`), by its file name. `None` when LibreOffice isn't installed or no such tool
/// works on this machine: conversions then run with the profile's settings alone. The first
/// call tries each tool on a small document of PdfCraft's own, which takes a few seconds.
#[cfg(not(target_arch = "wasm32"))]
pub fn libreoffice_sandbox() -> Option<String> {
    let sandbox = native::sandbox_for(&native::libreoffice()?)?;
    Some(sandbox.program.file_name()?.to_string_lossy().into_owned())
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{CreateError, MAX_PDF, office_extension};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, PoisonError};
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
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    }

    /// The fresh profile's settings: no macros at all, no links to untrusted locations, and
    /// linked content in Writer and Calc documents (sections, files, external data) never
    /// updated on load, so a document can't make LibreOffice fetch from the network or a share.
    /// The two `Link` settings number "Never" differently (Writer.xcs: 0 Always, 1 On request,
    /// 2 Never; Calc.xcs: 0 Always, 1 Never, 2 On request). Impress has no such setting, and
    /// a linked picture is fetched whatever these say, so web requests are also sent to a proxy
    /// that isn't there (`Inet/Settings`: type 1 is "manual"; port 9 is the discard service,
    /// which nothing listens on). LibreOffice doesn't use a proxy for this machine's own
    /// addresses, and the settings are its own to honour: [`Sandbox`] is what takes the network
    /// away.
    pub(super) const PROFILE_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="BlockUntrustedRefererLinks" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Writer/Content/Update"><prop oor:name="Link" oor:op="fuse"><value>2</value></prop></item>
<item oor:path="/org.openoffice.Office.Calc/Content/Update"><prop oor:name="Link" oor:op="fuse"><value>1</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetProxyType" oor:op="fuse"><value>1</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPProxyName" oor:op="fuse"><value>127.0.0.1</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPProxyPort" oor:op="fuse"><value>9</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPSProxyName" oor:op="fuse"><value>127.0.0.1</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPSProxyPort" oor:op="fuse"><value>9</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetNoProxy" oor:op="fuse"><value></value></prop></item>
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
            // Not recursive: a folder that already exists is not ours. Owner-only on Unix, where the
            // temporary folder is shared: the document and its PDF are not for other users.
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&dir)?;
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
        /// The watched folder grew past its limit while the command ran.
        TooLarge,
    }

    /// The bytes in the files directly inside `dir` (LibreOffice writes its one output there).
    /// Anything unreadable counts as nothing, and the sum saturates. Each size is read from the
    /// file itself: on Windows a directory listing keeps the old size while a process still
    /// has the file open for writing.
    pub(super) fn folder_size(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
        entries
            .flatten()
            .filter_map(|entry| std::fs::symlink_metadata(entry.path()).ok())
            .filter(|meta| meta.is_file())
            .fold(0u64, |sum, meta| sum.saturating_add(meta.len()))
    }

    /// Run `command` to completion, killing it and every process it started once `timeout`
    /// passes, or once the files in `watch`'s folder add up to more than its limit (so a
    /// document that converts to an enormous PDF can't fill the disk before it is checked).
    /// `soffice` hands the conversion to its own child, `soffice.bin`, which would
    /// otherwise outlive the kill, holding the profile folder and its memory.
    pub(super) fn run_limited(mut command: Command, timeout: Duration, watch: Option<(&Path, u64)>) -> std::io::Result<Run> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Its own process group, so the whole group can be stopped together.
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(Run::Exited(status)),
                Ok(None) => {}
                Err(error) => {
                    kill_tree(&mut child);
                    return Err(error);
                }
            }
            if watch.is_some_and(|(dir, limit)| folder_size(dir) > limit) {
                kill_tree(&mut child);
                return Ok(Run::TooLarge);
            }
            if start.elapsed() >= timeout {
                kill_tree(&mut child);
                return Ok(Run::TimedOut);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Kill `child` and every process it started, then reap it.
    fn kill_tree(child: &mut Child) {
        #[cfg(unix)]
        {
            use rustix::process::{Pid, Signal, kill_process_group};
            // The group `process_group(0)` made has the child's id.
            if let Some(group) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) {
                let _ = kill_process_group(group, Signal::KILL);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // `/T` walks the tree from the child's id, so it runs while the child is still alive.
            // Windows' own copy, not whatever `taskkill` the PATH finds first.
            let taskkill = std::env::var_os("SystemRoot")
                .map_or_else(|| PathBuf::from("taskkill.exe"), |root| PathBuf::from(root).join("System32").join("taskkill.exe"));
            let _ = Command::new(taskkill)
                .args(["/T", "/F", "/PID", &child.id().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                // CREATE_NO_WINDOW, as for LibreOffice itself.
                .creation_flags(0x0800_0000)
                .status();
        }
        // The child itself, if the tree kill couldn't reach it.
        let _ = child.kill();
        let _ = child.wait();
    }

    /// A program that runs another with no network: LibreOffice started through it can't reach
    /// a web server, a share or this machine's own services, whatever the document links to.
    /// The operating system enforces it, where the profile's settings are LibreOffice's own to
    /// honour.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) struct Sandbox {
        pub(super) program: PathBuf,
        /// What comes before the program to run.
        pub(super) args: &'static [&'static str],
    }

    impl Sandbox {
        /// `program` started inside the sandbox; its own arguments follow.
        pub(super) fn command(&self, program: &Path) -> Command {
            let mut command = Command::new(&self.program);
            command.args(self.args).arg(program);
            command
        }
    }

    /// macOS: everything allowed but the network.
    const SEATBELT_NO_NETWORK: &str = "(version 1)(allow default)(deny network*)";
    /// The same, for a LibreOffice that can't start without its sockets in the file system:
    /// those stay, and reach only this machine's own processes.
    const SEATBELT_LOCAL_SOCKETS: &str = "(version 1)(allow default)(deny network*)(allow network-outbound (remote unix-socket))(allow network-bind (local unix-socket))(allow network-inbound (local unix-socket))";

    /// The sandboxes `os` (as `std::env::consts::OS` names it) may have, the most contained
    /// first, given where a program of a name is. Whether one works is for [`sandbox_for`] to
    /// find out: user namespaces can be turned off, and a tool can be there but refused.
    ///
    /// - Linux: a network namespace of its own, holding only a loopback interface that is down.
    ///   bubblewrap keeps the file system as it is (`--dev-bind / /`) and goes with its parent;
    ///   util-linux's `unshare` does the same as the current user, or as root inside the
    ///   namespace where it can't map the user.
    /// - macOS: `sandbox-exec` with a profile that denies the network.
    /// - Windows and the BSDs have no such tool to run: nothing here.
    pub(super) fn sandboxes(os: &str, find: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<Sandbox> {
        let tools: &[(&str, &'static [&'static str])] = match os {
            "linux" => &[
                ("bwrap", &["--unshare-net", "--die-with-parent", "--dev-bind", "/", "/", "--"]),
                ("unshare", &["--user", "--map-current-user", "--net", "--"]),
                ("unshare", &["--user", "--map-root-user", "--net", "--"]),
            ],
            "macos" => &[("sandbox-exec", &["-p", SEATBELT_NO_NETWORK]), ("sandbox-exec", &["-p", SEATBELT_LOCAL_SOCKETS])],
            _ => &[],
        };
        tools.iter().filter_map(|(name, args)| Some(Sandbox { program: find(name)?, args })).collect()
    }

    /// Where the sandbox tool `name` is: macOS's own copy of `sandbox-exec`, anything else on
    /// the `PATH` (as LibreOffice itself is found).
    pub(super) fn sandbox_tool(name: &str) -> Option<PathBuf> {
        if name == "sandbox-exec" {
            return Some(PathBuf::from("/usr/bin/sandbox-exec")).filter(|p| p.is_file());
        }
        std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(name)).find(|p| p.is_file())
    }

    /// A small document of PdfCraft's own, converted to find out whether a sandbox works.
    const PROBE: &[u8] = b"{\\rtf1\\ansi{\\fonttbl\\f0\\fswiss Helvetica;}\\f0\\pard PdfCraft\\par}";

    /// How long a sandbox gets to convert [`PROBE`]: a fresh profile costs a few seconds.
    const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

    /// The first of `candidates` under which the LibreOffice at `program` converts [`PROBE`].
    /// Only a real conversion tells: a tool can start and LibreOffice still fail inside it.
    pub(super) fn working_sandbox(program: &Path, candidates: Vec<Sandbox>) -> Option<Sandbox> {
        candidates
            .into_iter()
            .find(|sandbox| convert_in(program, "probe.rtf", PROBE, PROBE_TIMEOUT, MAX_PDF, Some(sandbox)).is_ok_and(|pdf| pdf.starts_with(b"%PDF-")))
    }

    /// The sandbox the LibreOffice at `program` converts in, found once per program (the
    /// first conversion waits for it) and remembered: `None` when no candidate works here.
    pub(super) fn sandbox_for(program: &Path) -> Option<Sandbox> {
        static FOUND: Mutex<Option<(PathBuf, Option<Sandbox>)>> = Mutex::new(None);
        // A panic while probing leaves nothing half-written: the last answer still holds.
        let mut found = FOUND.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((known, sandbox)) = found.as_ref()
            && known == program
        {
            return sandbox.clone();
        }
        let sandbox = working_sandbox(program, sandboxes(std::env::consts::OS, &sandbox_tool));
        *found = Some((program.to_path_buf(), sandbox.clone()));
        sandbox
    }

    /// Convert with the LibreOffice at `program`, off the network where this machine has a
    /// sandbox for it ([`sandbox_for`]). Without one the conversion still runs, under the
    /// profile's settings alone, as it did before there were sandboxes.
    pub(super) fn convert(program: &Path, name: &str, bytes: &[u8]) -> Result<Vec<u8>, CreateError> {
        convert_in(program, name, bytes, super::TIMEOUT, MAX_PDF, sandbox_for(program).as_ref())
    }

    /// Convert with the LibreOffice at `program`, giving up after `timeout`. No sandbox.
    #[cfg(test)]
    pub(super) fn convert_with(program: &Path, name: &str, bytes: &[u8], timeout: Duration) -> Result<Vec<u8>, CreateError> {
        convert_capped(program, name, bytes, timeout, MAX_PDF)
    }

    /// [`convert_with`] with the converted PDF's size limit given.
    #[cfg(test)]
    pub(super) fn convert_capped(program: &Path, name: &str, bytes: &[u8], timeout: Duration, max_pdf: u64) -> Result<Vec<u8>, CreateError> {
        convert_in(program, name, bytes, timeout, max_pdf, None)
    }

    /// Convert with the LibreOffice at `program`, started inside `sandbox` when there is one.
    /// The output folder is watched while LibreOffice runs, so the PDF never grows past
    /// `max_pdf`, and the finished file is checked again.
    pub(super) fn convert_in(
        program: &Path,
        name: &str,
        bytes: &[u8],
        timeout: Duration,
        max_pdf: u64,
        sandbox: Option<&Sandbox>,
    ) -> Result<Vec<u8>, CreateError> {
        let too_large = || format!("the converted PDF is larger than {} MB", max_pdf / (1024 * 1024));
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

        let mut command = sandbox.map_or_else(|| Command::new(program), |sandbox| sandbox.command(program));
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
        let status = match run_limited(command, timeout, Some((&out, max_pdf))) {
            Ok(Run::Exited(status)) => status,
            Ok(Run::TooLarge) => return Err(failed(format!("{}; LibreOffice was stopped", too_large()))),
            Ok(Run::TimedOut) => return Err(failed(format!("LibreOffice took longer than {} seconds and was stopped", timeout.as_secs()))),
            Err(e) => return Err(failed(format!("couldn't start LibreOffice ({}): {e}", program.display()))),
        };
        let pdf = out.join("document.pdf");
        let size = match std::fs::metadata(&pdf) {
            Ok(meta) => meta.len(),
            Err(_) if !status.success() => return Err(failed(format!("LibreOffice couldn't convert it ({status})"))),
            Err(_) => return Err(failed("LibreOffice didn't produce a PDF; the file may be damaged or password-protected".into())),
        };
        if size > max_pdf {
            return Err(failed(too_large()));
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

        /// Each setting the fresh profile makes, as (path, property, value).
        fn profile_settings() -> Vec<(&'static str, &'static str, &'static str)> {
            let between = |s: &'static str, start: &str, end: &str| s.split_once(start).and_then(|(_, rest)| rest.split_once(end)).map(|(v, _)| v);
            PROFILE_SETTINGS
                .split("<item ")
                .skip(1)
                .filter_map(|item| {
                    Some((between(item, "oor:path=\"", "\"")?, between(item, "oor:name=\"", "\"")?, between(item, "<value>", "</value>")?))
                })
                .collect()
        }

        #[test]
        fn the_profile_turns_off_macros_and_never_updates_links() {
            let settings = profile_settings();
            let scripting = "/org.openoffice.Office.Common/Security/Scripting";
            assert!(settings.contains(&(scripting, "DisableMacrosExecution", "true")), "{settings:?}");
            assert!(settings.contains(&(scripting, "MacroSecurityLevel", "3")), "{settings:?}");
            assert!(settings.contains(&(scripting, "BlockUntrustedRefererLinks", "true")), "{settings:?}");
            // "Never" is 2 for Writer but 1 for Calc (Writer.xcs and Calc.xcs); Writer's 1 is
            // its default, "On request".
            assert!(settings.contains(&("/org.openoffice.Office.Writer/Content/Update", "Link", "2")), "{settings:?}");
            assert!(settings.contains(&("/org.openoffice.Office.Calc/Content/Update", "Link", "1")), "{settings:?}");
            // Web requests go to a proxy nothing listens on, with no host excepted.
            let inet = "/org.openoffice.Inet/Settings";
            for (name, value) in [("ooInetProxyType", "1"), ("ooInetHTTPProxyPort", "9"), ("ooInetHTTPSProxyPort", "9")] {
                assert!(settings.contains(&(inet, name, value)), "{name}: {settings:?}");
            }
            for name in ["ooInetHTTPProxyName", "ooInetHTTPSProxyName"] {
                assert!(settings.contains(&(inet, name, "127.0.0.1")), "{name}: {settings:?}");
            }
            assert!(settings.contains(&(inet, "ooInetNoProxy", "")), "{settings:?}");
            assert_eq!(settings.len(), 11, "{settings:?}");
        }

        #[test]
        fn each_system_offers_its_sandboxes_most_contained_first() {
            let everywhere = |name: &str| Some(PathBuf::from("/tools").join(name));
            let names = |os: &str, find: &dyn Fn(&str) -> Option<PathBuf>| -> Vec<(String, &'static str)> {
                sandboxes(os, find).iter().map(|s| (s.program.to_string_lossy().replace('\\', "/"), s.args[0])).collect()
            };
            assert_eq!(
                names("linux", &everywhere),
                [("/tools/bwrap".to_string(), "--unshare-net"), ("/tools/unshare".to_string(), "--user"), ("/tools/unshare".to_string(), "--user")]
            );
            // Each Linux sandbox takes the network away, and the program to run comes last.
            for sandbox in sandboxes("linux", &everywhere) {
                assert!(sandbox.args.iter().any(|a| matches!(*a, "--unshare-net" | "--net")), "{sandbox:?}");
                assert_eq!(sandbox.args.last(), Some(&"--"), "{sandbox:?}");
            }
            let mac = sandboxes("macos", &everywhere);
            assert_eq!(mac.len(), 2);
            assert!(mac.iter().all(|s| s.args[0] == "-p" && s.args[1].contains("(deny network*)")), "{mac:?}");
            // A tool that isn't installed is left out; other systems have none.
            assert_eq!(names("linux", &|name| (name == "unshare").then(|| PathBuf::from("/usr/bin/unshare"))).len(), 2);
            assert!(sandboxes("linux", &|_| None).is_empty());
            for os in ["windows", "freebsd", "openbsd", ""] {
                assert!(sandboxes(os, &everywhere).is_empty(), "{os}");
            }
        }

        #[test]
        fn a_sandbox_starts_the_program_after_its_own_arguments() {
            let sandbox = Sandbox { program: PathBuf::from("/usr/bin/unshare"), args: &["--user", "--net", "--"] };
            let command = sandbox.command(Path::new("/opt/soffice"));
            assert_eq!(command.get_program(), "/usr/bin/unshare");
            assert_eq!(command.get_args().collect::<Vec<_>>(), ["--user", "--net", "--", "/opt/soffice"]);
        }

        /// The port the stand-in below connects to, and where it writes what happened.
        const LISTENER_PORT: &str = "PDFCRAFT_TEST_OFFICE_LISTENER_PORT";

        /// This test binary, run through `sandbox` (or directly) as the stand-in that connects
        /// to `port`: what the stand-in wrote, `None` when it never ran (the sandbox tool isn't
        /// usable on this machine).
        fn connects_under(sandbox: Option<&Sandbox>, port: u16, dir: &Path) -> Option<String> {
            let report = dir.join("report");
            let _ = std::fs::remove_file(&report);
            let exe = std::env::current_exe().unwrap();
            let mut command = sandbox.map_or_else(|| std::process::Command::new(&exe), |sandbox| sandbox.command(&exe));
            command.args(["--ignored", "--exact", "office::tests::native::connects_to_the_listener", "--nocapture"]);
            command.env(LISTENER_PORT, port.to_string()).env(HEARTBEAT, &report);
            command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            assert!(matches!(run_limited(command, Duration::from_secs(60), None), Ok(Run::Exited(_))));
            std::fs::read_to_string(&report).ok()
        }

        /// A program started through a sandbox can't reach a listener on this machine, which a
        /// program started directly can: the sandbox is what stops a document's links, not
        /// LibreOffice's settings. Needs no LibreOffice; a sandbox tool that can't run here
        /// (none installed, or user namespaces turned off) is skipped.
        #[test]
        fn a_sandboxed_program_cannot_reach_a_listener() {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let work = WorkDir::new().unwrap();
            assert_eq!(connects_under(None, port, &work.0).as_deref(), Some("connected"), "the stand-in reaches the listener on its own");
            let mut ran = 0;
            for sandbox in sandboxes(std::env::consts::OS, &sandbox_tool) {
                match connects_under(Some(&sandbox), port, &work.0).as_deref() {
                    None => eprintln!("skipped: {sandbox:?} doesn't run here"),
                    Some(report) => {
                        assert_eq!(report, "refused", "{sandbox:?} left the network reachable");
                        ran += 1;
                    }
                }
            }
            if ran == 0 {
                eprintln!("skipped: no sandbox tool runs on this machine");
            }
        }

        /// The stand-in for a LibreOffice that fetches what a document links to (run only by
        /// the test above).
        #[test]
        #[ignore]
        fn connects_to_the_listener() {
            let (Some(port), Some(report)) = (std::env::var(LISTENER_PORT).ok().and_then(|p| p.parse::<u16>().ok()), std::env::var_os(HEARTBEAT))
            else {
                return;
            };
            let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            let reached = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)).is_ok();
            std::fs::write(report, if reached { "connected" } else { "refused" }).unwrap();
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
            assert!(matches!(run_limited(command, Duration::from_millis(500), None), Ok(Run::TimedOut)));
            assert!(started.elapsed() < Duration::from_secs(30));
        }

        /// The stand-in for a LibreOffice that never finishes (run only by the test above).
        #[test]
        #[ignore]
        fn sleeps_for_a_minute() {
            std::thread::sleep(Duration::from_secs(60));
        }

        /// Where the stand-ins below write their heartbeat.
        const HEARTBEAT: &str = "PDFCRAFT_TEST_OFFICE_HEARTBEAT";

        /// This test binary run as one of its own ignored tests, with the heartbeat file set.
        fn stand_in(test: &str, heartbeat: &Path) -> std::process::Command {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--ignored", "--exact", test, "--nocapture"]).env(HEARTBEAT, heartbeat);
            command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            command
        }

        /// `soffice` starts `soffice.bin` as its own child: a timeout must stop both, or the
        /// child keeps running, holding the profile folder.
        #[test]
        fn a_timeout_stops_the_processes_the_converter_started() {
            let work = WorkDir::new().unwrap();
            let heartbeat = work.0.join("heartbeat");
            let parent = stand_in("office::tests::native::starts_a_child_that_keeps_beating", &heartbeat);
            assert!(matches!(run_limited(parent, Duration::from_secs(5), None), Ok(Run::TimedOut)));
            let beats = || std::fs::metadata(&heartbeat).map_or(0, |m| m.len());
            // A killed process may take a moment to go.
            std::thread::sleep(Duration::from_millis(300));
            let after_kill = beats();
            assert!(after_kill > 0, "the grandchild started and was beating before the timeout");
            std::thread::sleep(Duration::from_secs(1));
            assert_eq!(beats(), after_kill, "the grandchild was stopped with its parent");
        }

        /// The stand-in for `soffice`: starts [`beats_until_stopped`] and waits (run only by the
        /// test above).
        #[test]
        #[ignore]
        fn starts_a_child_that_keeps_beating() {
            let Some(heartbeat) = std::env::var_os(HEARTBEAT) else { return };
            let mut child = stand_in("office::tests::native::beats_until_stopped", Path::new(&heartbeat)).spawn().unwrap();
            std::thread::sleep(Duration::from_secs(60));
            let _ = child.kill();
            let _ = child.wait();
        }

        /// The stand-in for `soffice.bin`: appends to the heartbeat file every 50 ms for a minute.
        #[test]
        #[ignore]
        fn beats_until_stopped() {
            use std::io::Write;
            let Some(heartbeat) = std::env::var_os(HEARTBEAT) else { return };
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(heartbeat).unwrap();
            for _ in 0..1200 {
                file.write_all(b".").unwrap();
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        /// Where the stand-in below writes.
        const FILL_DIR: &str = "PDFCRAFT_TEST_OFFICE_FILL_DIR";

        /// A converter that writes without end is stopped as soon as its output passes the
        /// limit, not after it has run out its time.
        #[test]
        fn a_converter_whose_output_grows_past_the_limit_is_stopped() {
            let work = WorkDir::new().unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--ignored", "--exact", "office::tests::native::writes_without_end", "--nocapture"]).env(FILL_DIR, &work.0);
            command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            let limit = 256 * 1024;
            let started = std::time::Instant::now();
            let run = run_limited(command, Duration::from_secs(60), Some((&work.0, limit)));
            assert!(matches!(run, Ok(Run::TooLarge)), "stopped for size, not for time");
            assert!(started.elapsed() < Duration::from_secs(30), "stopped long before the timeout");
            // A killed process may take a moment to go; after that nothing is written.
            std::thread::sleep(Duration::from_millis(300));
            let after_kill = folder_size(&work.0);
            assert!(after_kill > limit, "the stand-in had written past the limit");
            std::thread::sleep(Duration::from_secs(1));
            assert_eq!(folder_size(&work.0), after_kill, "the converter was stopped");
        }

        /// The stand-in for a LibreOffice that fills the disk: 64 KiB every 10 ms for a minute
        /// (run only by the test above).
        #[test]
        #[ignore]
        fn writes_without_end() {
            use std::io::Write;
            let Some(dir) = std::env::var_os(FILL_DIR) else { return };
            let mut file = std::fs::File::create(Path::new(&dir).join("document.pdf")).unwrap();
            let chunk = vec![0u8; 64 * 1024];
            for _ in 0..6000 {
                file.write_all(&chunk).unwrap();
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        #[test]
        fn a_folder_that_cannot_be_read_counts_as_empty() {
            assert_eq!(folder_size(&std::env::temp_dir().join("pdfcraft-no-such-folder")), 0);
        }

        /// The temporary folder holds a confidential document and its PDF: on Unix only the owner
        /// may enter it.
        #[cfg(unix)]
        #[test]
        fn the_temporary_folder_is_private_to_its_owner() {
            use std::os::unix::fs::PermissionsExt;
            let work = WorkDir::new().unwrap();
            let mode = std::fs::metadata(&work.0).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{mode:o}");
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

        /// With LibreOffice installed, each sandbox that works on this machine converts a
        /// document, and the one conversions use is the first of them.
        #[test]
        fn libreoffice_converts_inside_the_sandbox_this_machine_has() {
            let Some(program) = super::super::libreoffice() else {
                eprintln!("skipped: LibreOffice isn't installed");
                return;
            };
            let Some(sandbox) = sandbox_for(&program) else {
                eprintln!("skipped: no sandbox works on this machine");
                return;
            };
            let rtf = b"{\\rtf1\\ansi{\\fonttbl\\f0\\fswiss Helvetica;}\\f0\\pard Hello from PdfCraft.\\par}";
            let pdf = convert_in(&program, "hello.rtf", rtf, super::super::TIMEOUT, 1 << 20, Some(&sandbox)).unwrap();
            assert!(pdf.starts_with(b"%PDF-"));
            assert_eq!(sandbox_for(&program), Some(sandbox.clone()), "found once and remembered");
            assert_eq!(super::super::libreoffice_sandbox(), sandbox.program.file_name().map(|n| n.to_string_lossy().into_owned()));
            assert_eq!(convert(&program, "hello.rtf", rtf).map(|pdf| pdf.starts_with(b"%PDF-")), Ok(true));
        }

        /// A sandbox LibreOffice can't convert in is passed over, and none at all is an answer.
        #[test]
        fn a_sandbox_that_cannot_convert_is_not_used() {
            // This test binary stands in for LibreOffice: it makes no PDF, in or out of a sandbox.
            let program = std::env::current_exe().unwrap();
            let broken = Sandbox { program: std::env::temp_dir().join("pdfcraft-no-such-sandbox"), args: &["--"] };
            assert_eq!(working_sandbox(&program, vec![broken]), None);
            assert_eq!(working_sandbox(&program, Vec::new()), None);
        }
    }
}
