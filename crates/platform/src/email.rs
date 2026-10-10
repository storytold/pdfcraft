use std::io;
use std::path::Path;
use std::process::Command;

const SUBJECT: &str = "PdfCraft email";

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct EmailCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl EmailCommand {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn as_command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        cmd
    }

    /// Runs the command and waits. Errors if it can't start or exits non-zero.
    fn run(&self) -> io::Result<()> {
        let status = self.as_command().status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("{} exited with {}", self.program, status),
            ))
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendOutcome {
    /// A compose window with the attachment was opened.
    ComposeOpened,
    /// Composing failed; the file was revealed in the file manager instead.
    Revealed,
}

pub fn send_email_attachment(path: &str) -> io::Result<SendOutcome> {
    if !Path::new(path).is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("not a file: {path}")));
    }

    match compose_native(path) {
        Ok(()) => Ok(SendOutcome::ComposeOpened),
        Err(err) => {
            eprintln!("compose failed ({err}); revealing file instead");
            reveal_command(path)
                .map_err(|m| io::Error::new(io::ErrorKind::Unsupported, m))?
                .run_ignoring_status()?;
            Ok(SendOutcome::Revealed)
        }
    }
}

impl EmailCommand {
    // explorer.exe returns exit code 1 even on success, so don't trust the status.
    fn run_ignoring_status(&self) -> io::Result<()> {
        self.as_command().status().map(|_| ())
    }
}

// ───────────────────────── Fallback: reveal in file manager ─────────────────────────

pub fn reveal_command(path: &str) -> Result<EmailCommand, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(EmailCommand::new("open", &["-R", path]))
    }

    #[cfg(target_os = "windows")]
    {
        let win_path = path.replace('/', "\\");
        Ok(EmailCommand {
            program: "explorer".to_string(),
            args: vec![format!("/select,{win_path}")],
        })
    }

    #[cfg(target_os = "linux")]
    {
        let parent = Path::new(path)
            .parent()
            .and_then(|p| p.to_str())
            .filter(|p| !p.is_empty())
            .unwrap_or(".");
        Ok(EmailCommand::new("xdg-open", &[parent]))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = path;
        Err(format!("unsupported operating system: {}", std::env::consts::OS))
    }
}

// ───────────────────────────────── Linux ─────────────────────────────────

#[cfg(target_os = "linux")]
fn compose_native(path: &str) -> io::Result<()> {
    linux_command(path).run()
}

#[cfg(target_os = "linux")]
pub fn linux_command(path: &str) -> EmailCommand {
    EmailCommand::new("xdg-email", &["--subject", SUBJECT, "--attach", path])
}

// ───────────────────────────────── Windows ─────────────────────────────────
// Simple MAPI: opens a compose window in whatever client is the registered
// default MAPI handler (Outlook classic, Thunderbird, eM Client, ...).
// The new Outlook and the Windows Mail app don't implement MAPI, so this can
// fail there and we fall back to revealing the file.

#[cfg(target_os = "windows")]
mod mapi {
    use std::ffi::{c_void, OsStr};
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    #[repr(C)]
    struct MapiFileDescW {
        reserved: u32,
        flags: u32,
        position: u32,
        path_name: *const u16,
        file_name: *const u16,
        file_type: *mut c_void,
    }

    #[repr(C)]
    struct MapiMessageW {
        reserved: u32,
        subject: *const u16,
        note_text: *const u16,
        message_type: *const u16,
        date_received: *const u16,
        conversation_id: *const u16,
        flags: u32,
        originator: *mut c_void,
        recip_count: u32,
        recips: *mut c_void,
        file_count: u32,
        files: *mut MapiFileDescW,
    }

    #[link(name = "mapi32")]
    extern "system" {
        fn MAPISendMailW(
            session: usize,
            ui_param: usize,
            message: *mut MapiMessageW,
            flags: u32,
            reserved: u32,
        ) -> u32;
    }

    const MAPI_LOGON_UI: u32 = 0x0000_0001;
    const MAPI_DIALOG: u32 = 0x0000_0008;
    const NO_POSITION: u32 = 0xFFFF_FFFF;

    pub fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn send(path: &str, subject: &str) -> io::Result<()> {
        let path_w = wide(&path.replace('/', "\\"));
        let subject_w = wide(subject);

        let mut file = MapiFileDescW {
            reserved: 0,
            flags: 0,
            position: NO_POSITION,
            path_name: path_w.as_ptr(),
            file_name: null(),
            file_type: null_mut(),
        };
        let mut msg = MapiMessageW {
            reserved: 0,
            subject: subject_w.as_ptr(),
            note_text: null(),
            message_type: null(),
            date_received: null(),
            conversation_id: null(),
            flags: 0,
            originator: null_mut(),
            recip_count: 0,
            recips: null_mut(),
            file_count: 1,
            files: &mut file,
        };

        // path_w / subject_w / file / msg all outlive this call.
        let rc = unsafe { MAPISendMailW(0, 0, &mut msg, MAPI_LOGON_UI | MAPI_DIALOG, 0) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::Other, format!("MAPISendMailW failed with code {rc}")))
        }
    }
}

#[cfg(target_os = "windows")]
fn compose_native(path: &str) -> io::Result<()> {
    mapi::send(path, SUBJECT)
}

// ───────────────────────────────── macOS ─────────────────────────────────
// 1. Ask LaunchServices which app handles mailto: (via JXA, no deps).
// 2. Dispatch to a per-client strategy. Paths go in as argv / URL-encoded,
//    never spliced into script text.

#[cfg(target_os = "macos")]
fn compose_native(path: &str) -> io::Result<()> {
    let bundle_id = default_mail_bundle_id()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no default mail app found"))?;
    let cmd = mac_compose_command(&bundle_id, path).ok_or_else(|| {
        io::Error::new(io::ErrorKind::Unsupported, format!("no attach strategy for {bundle_id}"))
    })?;
    cmd.run()
}

#[cfg(target_os = "macos")]
fn default_mail_bundle_id() -> Option<String> {
    const JXA: &str = r#"
        ObjC.import("AppKit");
        var u = $.NSWorkspace.sharedWorkspace.URLForApplicationToOpenURL($.NSURL.URLWithString("mailto:a@b.c"));
        ObjC.unwrap($.NSBundle.bundleWithURL(u).bundleIdentifier)
    "#;
    let out = Command::new("osascript").args(["-l", "JavaScript", "-e", JXA]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!id.is_empty() && id != "undefined").then_some(id)
}

const MAIL_APP_SCRIPT: &str = r#"on run argv
    set p to POSIX file (item 1 of argv)
    tell application "Mail"
        set msg to make new outgoing message with properties {subject:"PdfCraft email", visible:true}
        tell msg
            make new attachment with properties {file name:p} at after the last paragraph
        end tell
        activate
    end tell
end run"#;

const OUTLOOK_SCRIPT: &str = r#"on run argv
    set p to (POSIX file (item 1 of argv)) as alias
    tell application "Microsoft Outlook"
        set msg to make new outgoing message with properties {subject:"PdfCraft email"}
        make new attachment at msg with properties {file:p}
        open msg
        activate
    end tell
end run"#;

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn mac_compose_command(bundle_id: &str, path: &str) -> Option<EmailCommand> {
    match bundle_id {
        "com.apple.mail" => Some(EmailCommand {
            program: "osascript".into(),
            args: vec!["-e".into(), MAIL_APP_SCRIPT.into(), path.into()],
        }),
        "com.microsoft.Outlook" => Some(EmailCommand {
            program: "osascript".into(),
            args: vec!["-e".into(), OUTLOOK_SCRIPT.into(), path.into()],
        }),
        "org.mozilla.thunderbird" => Some(EmailCommand {
            program: "/Applications/Thunderbird.app/Contents/MacOS/thunderbird".into(),
            args: vec![
                "-compose".into(),
                format!("attachment='file://{}'", percent_encode_path(path)),
            ],
        }),
        _ => None, // Spark, Airmail, Gmail PWA, etc: no scriptable attach → fallback
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ───────────────────────────────── Other OS ─────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn compose_native(_path: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("unsupported operating system: {}", std::env::consts::OS),
    ))
}

// ───────────────────────────────── Tests ─────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_uses_xdg_email_attach() {
        let c = linux_command("/tmp/example.pdf");
        assert_eq!(c.program, "xdg-email");
        assert!(c.args.windows(2).any(|w| w == ["--attach", "/tmp/example.pdf"]));
    }

    #[test]
    fn mac_mail_and_outlook_pass_path_as_argv_not_in_script() {
        let nasty = r#"/tmp/a "b" 'c'.pdf"#;
        for id in ["com.apple.mail", "com.microsoft.Outlook"] {
            let c = mac_compose_command(id, nasty).unwrap();
            assert_eq!(c.program, "osascript");
            assert_eq!(c.args[0], "-e");
            assert!(!c.args[1].contains(nasty), "path must not be embedded in script");
            assert_eq!(c.args[2], nasty);
            assert!(c.args[1].contains("on run argv"));
        }
    }

    #[test]
    fn mac_thunderbird_encodes_path() {
        let c = mac_compose_command("org.mozilla.thunderbird", "/tmp/my file's.pdf").unwrap();
        assert_eq!(c.args[0], "-compose");
        assert_eq!(c.args[1], "attachment='file:///tmp/my%20file%27s.pdf'");
    }

    #[test]
    fn mac_unknown_client_has_no_strategy() {
        assert!(mac_compose_command("com.readdle.smartemail-Mac", "/tmp/x.pdf").is_none());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_wide_string_is_nul_terminated() {
        let w = mapi::wide("C:\\tmp\\x.pdf");
        assert_eq!(*w.last().unwrap(), 0);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_reveal_uses_backslashes() {
        let c = reveal_command("C:/tmp/example.pdf").unwrap();
        assert_eq!(c.program, "explorer");
        assert_eq!(c.args, ["/select,C:\\tmp\\example.pdf"]);
    }

    #[test]
    fn nothing_uses_mailto() {
        for id in ["com.apple.mail", "com.microsoft.Outlook", "org.mozilla.thunderbird"] {
            let c = mac_compose_command(id, "/tmp/example.pdf").unwrap();
            assert!(!format!("{} {}", c.program, c.args.join(" ")).contains("mailto:"));
        }
    }
}