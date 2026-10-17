//! Send by email: a new message in the computer's default email program with a file attached,
//! the way Acrobat's *Attach to Email* works. Nothing is sent from here and nothing goes over the
//! network: the message opens in the person's own email program for them to address and send.
//!
//! Each system is asked through programs it already has (no unsafe code, no new dependencies):
//!
//! * **macOS:** the default handler for `mailto:` links (Launch Services, read through
//!   `osascript -l JavaScript`). Apple Mail and Microsoft Outlook are scriptable, so the file is
//!   attached with AppleScript; any other default gets the fallback.
//! * **Windows:** when the default for `mailto:` links is the new Outlook (`olk.exe`), which does
//!   not take Simple MAPI requests, an unsent message (`.eml` with `X-Unsent: 1`, the file inside)
//!   that it opens as a new one. Otherwise Simple MAPI's `MAPISendMailW`, which reaches whichever
//!   program is set as the default MAPI client (classic Outlook, Thunderbird, eM Client…), through
//!   a PowerShell script that watches for the compose window. No MAPI client: the fallback.
//! * **Linux and the BSDs:** `xdg-email --attach`, which asks the desktop's default email
//!   program (Thunderbird, Evolution, KMail…). No `xdg-email`: the fallback.
//! * **The web build:** [`MailError::Unsupported`].
//!
//! The **fallback**, for a default that can't be handed a file (a webmail site in a browser, an
//! unknown program, no MAPI client) or a program that didn't open a message: a `mailto:` address
//! with the recipients, subject and body, to open, and the file to show in its folder so the person
//! can attach it themselves. [`send`] never opens it by itself; it reports it in
//! [`Answer::NotAttached`] for the caller to offer, so nothing fails without a word.
//!
//! [`detect`] reads the default (it runs the system's own tools); [`plan_for`] turns a default into
//! what would run and is pure, so every system's plans are tested on any machine; [`send`] does
//! both on a thread and runs the plan.

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

/// How long a scripted request (AppleScript, Simple MAPI) may take before it counts as no answer.
/// The first AppleScript request waits on the system's "control Mail?" prompt, and PowerShell
/// compiles its MAPI helper before asking.
pub const ANSWER_SECONDS: u64 = 60;

/// How long a Simple MAPI request may take to show a compose window. A MAPI client started
/// without a window for a profile nobody uses can stay logging on for ever.
pub const MAPI_SECONDS: u32 = 30;

/// How long `xdg-email` is watched for an early failure. It can stay running for as long as the
/// email program it started (Thunderbird, when it wasn't running yet), so still running then
/// counts as opened.
const SETTLE_SECONDS: u64 = 5;

/// The largest file put inside a message file for the new Outlook (it is read into memory and
/// encoded). Mail services refuse much smaller attachments anyway.
pub const MAX_MESSAGE_FILE_BYTES: u64 = 150 * 1024 * 1024;

/// What to put in the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compose {
    /// The file to attach.
    pub path: PathBuf,
    pub subject: String,
    pub body: String,
    /// Recipients' addresses; empty leaves the To line blank.
    pub to: Vec<String>,
}

/// The computer's default email program, as [`detect`] found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultMail {
    /// macOS: Apple Mail.
    AppleMail,
    /// macOS: Microsoft Outlook.
    Outlook,
    /// Windows: the new Outlook, started through this `olk.exe`.
    NewOutlook(PathBuf),
    /// Windows: a Simple MAPI client, by the name it is registered under.
    Mapi(String),
    /// Linux and the BSDs: this `xdg-email`.
    XdgEmail(PathBuf),
    /// A program that can't be handed a file from here (a browser for a webmail site, an
    /// unknown email program), by its name when the system gives one.
    Other(Option<String>),
    /// None is set up.
    Nothing,
}

/// What [`send`] would do for a default ([`plan_for`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The email program's name, when it is known ("Mail", "Mozilla Thunderbird").
    pub program: Option<String>,
    pub route: Route,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// Run a helper that puts the file in a new message.
    Run {
        command: PathBuf,
        args: Vec<String>,
        /// Written to the helper's standard input (an AppleScript).
        stdin: Option<String>,
        watch: Watch,
    },
    /// Write an unsent message with the file inside to `message`, and open it with `program`
    /// (the new Outlook).
    MessageFile { program: PathBuf, message: PathBuf },
    /// The file can't be handed over: offer the fallback.
    Fallback { why: Why, fallback: Fallback },
}

/// How a helper says it worked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Watch {
    /// Its exit status (osascript), within [`ANSWER_SECONDS`].
    Exit,
    /// One answer line on its standard output (the Simple MAPI script), within [`ANSWER_SECONDS`].
    Answers,
    /// A failure shows as an early exit; still running after a few seconds is success
    /// (xdg-email, which may run for as long as the email program it started).
    Started,
}

/// A message without the file, and the file to attach by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fallback {
    /// A `mailto:` address with the recipients, subject and body ([`mailto`]).
    pub mailto: String,
    /// The file, to show in its folder.
    pub reveal: PathBuf,
}

/// What came of [`send`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// A new message is open in the email program with the file attached (or the person has
    /// already sent or closed it).
    Opened { program: Option<String> },
    /// The file wasn't attached; the fallback is what can be offered instead.
    NotAttached { program: Option<String>, why: Why, fallback: Fallback },
}

/// Why the file wasn't attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// The default email program can't be handed a file from here.
    CantAttach,
    /// No email program is set up.
    NoProgram,
    /// The program didn't open a message in time.
    NoAnswer,
    /// The program couldn't log on to its mailbox (Simple MAPI's `MAPI_E_LOGIN_FAILURE`).
    LogonFailed,
    /// Anything else, in the program's or the system's words.
    Failed(String),
}

/// Why [`send`] refused before anything ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MailError {
    /// The file to attach is not a file that exists.
    NoFile(PathBuf),
    /// A recipient is empty, starts with `-` or contains a control character.
    BadRecipient(String),
    /// No email program can be asked on this system (the web build).
    Unsupported,
    /// The thread that asks could not be started.
    CouldNotStart(String),
}

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFile(p) => write!(f, "the file to attach is missing: {}", p.display()),
            Self::BadRecipient(a) => write!(f, "“{}” isn't an email address", a.escape_debug()),
            Self::Unsupported => write!(f, "sending by email isn't available on this system"),
            Self::CouldNotStart(why) => write!(f, "the email program could not be asked: {why}"),
        }
    }
}

impl std::error::Error for MailError {}

/// Refuse what can't be sent before anything runs: a file that isn't there, a recipient that
/// isn't an address a command line or a message header can carry as it is.
pub fn check(compose: &Compose) -> Result<(), MailError> {
    if !compose.path.is_file() {
        return Err(MailError::NoFile(compose.path.clone()));
    }
    if let Some(bad) = compose.to.iter().find(|a| a.trim().is_empty() || a.starts_with('-') || a.chars().any(char::is_control)) {
        return Err(MailError::BadRecipient(bad.clone()));
    }
    Ok(())
}

/// Attach `compose`'s file to a new message in the default email program, on a thread: the
/// receiver gets one [`Answer`], and `wake` is called once it is there. A missing file or a bad
/// recipient is refused here, before anything runs.
pub fn send(compose: Compose, wake: impl FnOnce() + Send + 'static) -> Result<Receiver<Answer>, MailError> {
    if cfg!(target_arch = "wasm32") {
        return Err(MailError::Unsupported);
    }
    check(&compose)?;
    let mut compose = compose;
    // Absolute, so a helper can't read a file name that starts with '-' as an option.
    compose.path = std::path::absolute(&compose.path).map_err(|_| MailError::NoFile(compose.path.clone()))?;
    let os = std::env::consts::OS;
    // Unsupported systems are refused here rather than on the thread.
    plan_for(os, &DefaultMail::Nothing, &compose)?;
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("pdfcraft-mail".into())
        .spawn(move || {
            let answer = match plan_for(os, &detect(), &compose) {
                Ok(plan) => execute(plan, &compose),
                // Not reached: the system was checked above.
                Err(e) => not_attached(None, Why::Failed(e.to_string()), &compose),
            };
            let _ = tx.send(answer);
            wake();
        })
        .map_err(|e| MailError::CouldNotStart(e.to_string()))?;
    Ok(rx)
}

/// The plan for `compose` on system `os` ("macos", "windows", "linux", …, as
/// [`std::env::consts::OS`] names them) when its default email program is `default`. A default
/// that doesn't belong to `os` is treated as one that can't be handed a file.
pub fn plan_for(os: &str, default: &DefaultMail, compose: &Compose) -> Result<Plan, MailError> {
    let path = compose.path.to_string_lossy().into_owned();
    let offer = |program: Option<String>, why: Why| Plan { program, route: Route::Fallback { why, fallback: fallback(compose) } };
    match (os, default) {
        ("macos" | "windows" | "linux" | "freebsd" | "openbsd" | "netbsd" | "dragonfly", DefaultMail::Nothing) => Ok(offer(None, Why::NoProgram)),
        ("macos", DefaultMail::AppleMail | DefaultMail::Outlook) => {
            let (program, script) = if *default == DefaultMail::Outlook {
                ("Microsoft Outlook", outlook_script(&path, compose))
            } else {
                ("Mail", mail_app_script(&path, compose))
            };
            Ok(Plan {
                program: Some(program.into()),
                route: Route::Run { command: "/usr/bin/osascript".into(), args: vec!["-".into()], stdin: Some(script), watch: Watch::Exit },
            })
        }
        ("windows", DefaultMail::NewOutlook(olk)) => Ok(Plan {
            program: Some("Outlook".into()),
            route: Route::MessageFile { program: olk.clone(), message: message_dir().join(message_name(&compose.path)) },
        }),
        ("windows", DefaultMail::Mapi(name)) => Ok(Plan {
            program: Some(name.clone()),
            route: Route::Run {
                command: system_program("WindowsPowerShell\\v1.0\\powershell.exe"),
                args: powershell_args(&mapi_script(compose, std::process::id())),
                stdin: None,
                watch: Watch::Answers,
            },
        }),
        ("linux" | "freebsd" | "openbsd" | "netbsd" | "dragonfly", DefaultMail::XdgEmail(xdg)) => {
            let mut args = vec!["--utf8".into(), "--subject".into(), compose.subject.clone(), "--body".into(), compose.body.clone()];
            args.extend(["--attach".into(), path]);
            args.extend(compose.to.iter().cloned());
            Ok(Plan { program: None, route: Route::Run { command: xdg.clone(), args, stdin: None, watch: Watch::Started } })
        }
        ("macos" | "windows" | "linux" | "freebsd" | "openbsd" | "netbsd" | "dragonfly", other) => {
            let name = match other {
                DefaultMail::Other(name) => name.clone(),
                _ => None,
            };
            Ok(offer(name, Why::CantAttach))
        }
        _ => Err(MailError::Unsupported),
    }
}

/// The fallback for `compose`: a message without the file, and the file to show.
pub fn fallback(compose: &Compose) -> Fallback {
    Fallback { mailto: mailto(&compose.to, &compose.subject, &compose.body), reveal: compose.path.clone() }
}

fn not_attached(program: Option<String>, why: Why, compose: &Compose) -> Answer {
    Answer::NotAttached { program, why, fallback: fallback(compose) }
}

/// A `mailto:` address (RFC 6068): the recipients comma-separated, then `subject` and `body` when
/// they aren't empty. Everything but letters, digits and `-._~` (and `@` in an address) is
/// percent-encoded as UTF-8, so commas, `&`, `=`, `+`, spaces and non-ASCII text arrive as typed;
/// line breaks in the body are sent as CR LF.
pub fn mailto(to: &[String], subject: &str, body: &str) -> String {
    let mut url = String::from("mailto:");
    let addresses: Vec<String> = to.iter().map(|a| percent_encode(a.trim(), "@")).collect();
    url.push_str(&addresses.join(","));
    let body = body.replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\r\n");
    let mut separator = '?';
    for (key, value) in [("subject", subject), ("body", body.as_str())] {
        if !value.is_empty() {
            url.push(separator);
            url.push_str(key);
            url.push('=');
            url.push_str(&percent_encode(value, ""));
            separator = '&';
        }
    }
    url
}

fn percent_encode(text: &str, also_keep: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') || also_keep.as_bytes().contains(&b) {
            out.push(char::from(b));
        } else {
            use std::fmt::Write as _;
            // Writing to a String can't fail.
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Finding the default
// ---------------------------------------------------------------------------------------------

/// Find this computer's default email program. Runs the system's own tools (osascript on macOS,
/// reg on Windows); on Linux and the BSDs it looks for `xdg-email` on `PATH`.
pub fn detect() -> DefaultMail {
    match std::env::consts::OS {
        "macos" => {
            let out = output(Command::new("/usr/bin/osascript").args(["-l", "JavaScript", "-e", MACOS_DEFAULT]));
            out.map_or(DefaultMail::Nothing, |text| macos_default(&text))
        }
        "windows" => detect_windows(),
        "linux" | "freebsd" | "openbsd" | "netbsd" | "dragonfly" => {
            std::env::var_os("PATH").and_then(|p| find_program(&p, "xdg-email")).map_or(DefaultMail::Nothing, DefaultMail::XdgEmail)
        }
        _ => DefaultMail::Nothing,
    }
}

/// JavaScript for Automation: the application Launch Services opens `mailto:` links with, as
/// `id=`, `name=` and `path=` lines (nothing when there is none).
const MACOS_DEFAULT: &str = "ObjC.import('AppKit');
var url = $.NSWorkspace.sharedWorkspace.URLForApplicationToOpenURL($.NSURL.URLWithString('mailto:'));
var out = '';
if (!url.isNil()) {
  var bundle = $.NSBundle.bundleWithURL(url);
  var id = bundle.isNil() ? '' : (ObjC.unwrap(bundle.bundleIdentifier) || '');
  var name = bundle.isNil() ? '' : (ObjC.unwrap(bundle.objectForInfoDictionaryKey('CFBundleDisplayName')) || ObjC.unwrap(bundle.objectForInfoDictionaryKey('CFBundleName')) || '');
  out = 'id=' + id + '\\nname=' + name + '\\npath=' + ObjC.unwrap(url.path);
}
out;";

/// [`MACOS_DEFAULT`]'s answer as a default.
fn macos_default(text: &str) -> DefaultMail {
    let field = |key: &str| text.lines().find_map(|l| l.trim().strip_prefix(key)).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    let (id, name, path) = (field("id="), field("name="), field("path="));
    match id.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("com.apple.mail") => DefaultMail::AppleMail,
        Some("com.microsoft.outlook") => DefaultMail::Outlook,
        _ if id.is_none() && path.is_none() => DefaultMail::Nothing,
        _ => DefaultMail::Other(name.or(id)),
    }
}

fn detect_windows() -> DefaultMail {
    let reg = system_program("reg.exe");
    let query = |args: &[&str]| output(Command::new(&reg).arg("query").args(args));
    let key = "HKCU\\Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\mailto";
    let prog_id = query(&[&format!("{key}\\UserChoiceLatest\\ProgId"), "/v", "ProgId"])
        .or_else(|| query(&[&format!("{key}\\UserChoice"), "/v", "ProgId"]))
        .and_then(|out| reg_value(&out));
    // Everything registered under the ProgId: a packaged app names itself in its AppUserModelID,
    // a desktop app in its open command.
    let handler = prog_id.as_deref().filter(|p| !p.is_empty() && !p.contains(['\\', '"'])).and_then(|p| query(&[&format!("HKCR\\{p}"), "/s"]));
    let mut mapi = None;
    for hive in ["HKCU", "HKLM"] {
        let Some(name) = query(&[&format!("{hive}\\Software\\Clients\\Mail"), "/ve"]).and_then(|out| reg_value(&out)) else { continue };
        // A name that isn't the name of a registered client is a leftover or "(value not set)".
        if !name.contains('\\') && query(&[&format!("{hive}\\Software\\Clients\\Mail\\{name}"), "/ve"]).is_some() {
            mapi = Some(name);
            break;
        }
    }
    let olk = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Microsoft\\WindowsApps\\olk.exe")).filter(|p| p.exists());
    windows_default(prog_id.as_deref(), handler.as_deref(), mapi, olk)
}

/// Windows' default from what the registry says: the `mailto:` handler's ProgId and what is
/// registered under it, the default Simple MAPI client, and the new Outlook's `olk.exe` if it is
/// installed. The new Outlook first when it handles `mailto:` (it takes no MAPI requests); else the
/// MAPI client, which is the program Windows hands a file to send; else the handler, which can't
/// be handed a file from here.
fn windows_default(prog_id: Option<&str>, handler: Option<&str>, mapi: Option<String>, olk: Option<PathBuf>) -> DefaultMail {
    let names_new_outlook = |text: &str| {
        let text = text.to_ascii_lowercase();
        text.contains("outlookforwindows")
            || text
                .match_indices("olk.exe")
                .any(|(i, _)| i.checked_sub(1).and_then(|j| text.as_bytes().get(j)).is_none_or(|b| !b.is_ascii_alphanumeric()))
    };
    if [prog_id, handler].into_iter().flatten().any(names_new_outlook)
        && let Some(olk) = olk
    {
        return DefaultMail::NewOutlook(olk);
    }
    if let Some(name) = mapi.filter(|n| !n.trim().is_empty()) {
        return DefaultMail::Mapi(name.trim().to_string());
    }
    if prog_id.is_some_and(|p| !p.trim().is_empty()) { DefaultMail::Other(None) } else { DefaultMail::Nothing }
}

/// The value in `reg query`'s answer for one value: the text after its type on the first line
/// that has one. The value's own name is in the system's language ("(Default)"), so it isn't read.
fn reg_value(out: &str) -> Option<String> {
    out.lines().find_map(|line| {
        let (_, after) = line.split_once("REG_SZ").or_else(|| line.split_once("REG_EXPAND_SZ"))?;
        let value = after.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// `name` in one of `path`'s folders (`PATH`'s form), if it is a file there.
fn find_program(path: &std::ffi::OsStr, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path).filter(|dir| dir.is_absolute()).map(|dir| dir.join(name)).find(|p| p.is_file())
}

/// A program in Windows' System32 folder, by full path, so one beside PdfCraft can't stand in.
fn system_program(name: &str) -> PathBuf {
    match std::env::var_os("SystemRoot") {
        Some(root) => PathBuf::from(root).join("System32").join(name),
        None => PathBuf::from(name.rsplit('\\').next().unwrap_or(name)),
    }
}

/// A command's standard output when it ran and succeeded.
fn output(command: &mut Command) -> Option<String> {
    command.stdin(Stdio::null()).stderr(Stdio::null());
    hide_console(command);
    let out = command.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------------------------------------------------------------------------------------------
// Running a plan
// ---------------------------------------------------------------------------------------------

fn execute(plan: Plan, compose: &Compose) -> Answer {
    let Plan { program, route } = plan;
    let why = match route {
        Route::Fallback { why, fallback } => return Answer::NotAttached { program, why, fallback },
        Route::MessageFile { program: olk, message } => open_message_file(&olk, &message, compose),
        Route::Run { command, args, stdin, watch } => run(&command, &args, stdin.as_deref(), watch),
    };
    match why {
        None => Answer::Opened { program },
        Some(why) => not_attached(program, why, compose),
    }
}

/// Run a helper and watch it as `watch` says. `None` when it worked.
fn run(program: &Path, args: &[String], stdin: Option<&str>, watch: Watch) -> Option<Why> {
    let mut command = Command::new(program);
    command.args(args).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    command.stdout(if watch == Watch::Answers { Stdio::piped() } else { Stdio::null() });
    command.stderr(if watch == Watch::Exit { Stdio::piped() } else { Stdio::null() });
    hide_console(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return Some(Why::Failed(format!("{} could not be started: {e}", program.display()))),
    };
    if let (Some(script), Some(mut input)) = (stdin, child.stdin.take()) {
        use std::io::Write;
        // A script the helper doesn't read shows as its exit status.
        let _ = input.write_all(script.as_bytes());
    }
    match watch {
        Watch::Exit => match wait(&mut child, Duration::from_secs(ANSWER_SECONDS)) {
            Some(true) => None,
            Some(false) => {
                let mut said = String::new();
                if let Some(err) = child.stderr.take() {
                    let _ = err.take(4096).read_to_string(&mut said);
                }
                let said = said.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("it stopped with an error");
                Some(Why::Failed(said.to_string()))
            }
            None => {
                let _ = child.kill();
                reap(child);
                Some(Why::NoAnswer)
            }
        },
        Watch::Started => match wait(&mut child, Duration::from_secs(SETTLE_SECONDS)) {
            Some(true) => None,
            Some(false) => Some(Why::Failed(format!("{} stopped with an error", program.display()))),
            None => {
                reap(child);
                None
            }
        },
        Watch::Answers => watch_answers(child),
    }
}

/// Wait up to `limit` for `child` to stop: whether it succeeded, or `None` while it still runs.
fn wait(child: &mut Child, limit: Duration) -> Option<bool> {
    let until = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => return None,
            Err(_) => return Some(false),
        }
    }
}

/// Reap a child left running on a thread, so it never lingers as a zombie.
fn reap(mut child: Child) {
    let _ = std::thread::Builder::new().name("pdfcraft-mail-reap".into()).spawn(move || {
        let _ = child.wait();
    });
}

/// The Simple MAPI script's first answer line, within [`ANSWER_SECONDS`].
fn watch_answers(mut child: Child) -> Option<Why> {
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        reap(child);
        return Some(Why::Failed("the email helper's answer could not be read".into()));
    };
    let (line_tx, line_rx) = channel();
    let reader = std::thread::Builder::new().name("pdfcraft-mail-read".into()).spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Some(answer) = mapi_answer(&line) {
                let _ = line_tx.send(answer);
                return;
            }
        }
    });
    if reader.is_err() {
        let _ = child.kill();
        reap(child);
        return Some(Why::Failed("the email helper's answer could not be read".into()));
    }
    let answer = match line_rx.recv_timeout(Duration::from_secs(ANSWER_SECONDS)) {
        Ok(answer) => answer,
        Err(RecvTimeoutError::Timeout) => {
            let _ = child.kill();
            Some(Why::NoAnswer)
        }
        Err(RecvTimeoutError::Disconnected) => Some(Why::Failed("the email helper stopped without an answer".into())),
    };
    // A MAPI compose window keeps the script running until it is closed.
    reap(child);
    answer
}

/// The MAPI script's line as an answer: `opened`, `hung`, `code <MAPI result>` or
/// `failed <why>` (`Some(None)` is opened); anything else (PowerShell's own noise) is not one.
fn mapi_answer(line: &str) -> Option<Option<Why>> {
    let line = line.trim();
    match line {
        "opened" => return Some(None),
        "hung" => return Some(Some(Why::NoAnswer)),
        _ => {}
    }
    if let Some(code) = line.strip_prefix("code ") {
        let code: i64 = code.trim().parse().ok()?;
        return Some(match code {
            // SUCCESS_SUCCESS and MAPI_USER_ABORT: the message was shown (and sent or closed).
            0 | 1 => None,
            3 => Some(Why::LogonFailed),
            11 => Some(Why::Failed("the file to attach was not found".into())),
            n => Some(Why::Failed(format!("the email program answered MAPI error {n}"))),
        });
    }
    line.strip_prefix("failed ").map(|why| Some(Why::Failed(why.trim().to_string())))
}

/// Write the unsent message for the new Outlook and open it there.
fn open_message_file(olk: &Path, message: &Path, compose: &Compose) -> Option<Why> {
    let bytes = match std::fs::metadata(&compose.path) {
        Ok(m) if m.len() > MAX_MESSAGE_FILE_BYTES => {
            return Some(Why::Failed(format!("the file is larger than {} MB", MAX_MESSAGE_FILE_BYTES / (1024 * 1024))));
        }
        Ok(_) => match std::fs::read(&compose.path) {
            Ok(bytes) => bytes,
            Err(e) => return Some(Why::Failed(e.to_string())),
        },
        Err(e) => return Some(Why::Failed(e.to_string())),
    };
    let written = message
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(message, unsent_message(compose, &file_name(&compose.path), &bytes)));
    if let Err(e) = written {
        return Some(Why::Failed(format!("the message could not be written: {e}")));
    }
    let mut command = Command::new(olk);
    command.arg(message).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    match command.spawn() {
        Ok(child) => {
            reap(child);
            None
        }
        Err(e) => Some(Why::Failed(format!("{} could not be started: {e}", olk.display()))),
    }
}

/// Where unsent messages for the new Outlook are written.
fn message_dir() -> PathBuf {
    std::env::temp_dir().join("pdfcraft-send-email")
}

/// The message file's name: the attachment's, ending in `.eml`.
fn message_name(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = if stem.trim().is_empty() { "message".to_string() } else { stem };
    format!("{stem}.eml")
}

/// A file's name, splitting on both separators so the result is the same wherever this is built.
fn file_name(path: &Path) -> String {
    path.to_string_lossy().rsplit(['\\', '/']).next().unwrap_or_default().to_string()
}

/// An unsent message (RFC 5322 with MIME, `X-Unsent: 1` so it opens as a new one): the
/// recipients, the subject and the file's name as UTF-8 encoded words, the body and the file in
/// base64. Recipients were checked for control characters ([`check`]).
fn unsent_message(compose: &Compose, name: &str, file: &[u8]) -> String {
    let word = |t: &str| format!("=?UTF-8?B?{}?=", base64(t.as_bytes()));
    let boundary = format!("pdfcraft-{:016x}", {
        use std::hash::BuildHasher;
        std::hash::RandomState::new().hash_one(name)
    });
    let kind = if name.to_ascii_lowercase().ends_with(".pdf") { "application/pdf" } else { "application/octet-stream" };
    let mut lines = Vec::new();
    if !compose.to.is_empty() {
        lines.push(format!("To: {}", compose.to.iter().map(|a| a.trim()).collect::<Vec<_>>().join(", ")));
    }
    lines.push(format!("Subject: {}", word(&compose.subject)));
    lines.extend(["X-Unsent: 1".into(), "MIME-Version: 1.0".into(), format!("Content-Type: multipart/mixed; boundary=\"{boundary}\"")]);
    lines.extend([String::new(), format!("--{boundary}")]);
    lines.extend(["Content-Type: text/plain; charset=utf-8".into(), "Content-Transfer-Encoding: base64".into(), String::new()]);
    lines.push(base64_lines(compose.body.as_bytes()));
    lines.push(format!("--{boundary}"));
    lines.push(format!("Content-Type: {kind}; name=\"{}\"", word(name)));
    lines.push("Content-Transfer-Encoding: base64".into());
    lines.push(format!("Content-Disposition: attachment; filename=\"{}\"; filename*=UTF-8''{}", word(name), percent_encode(name, "")));
    lines.extend([String::new(), base64_lines(file), format!("--{boundary}--"), String::new()]);
    lines.join("\r\n")
}

// ---------------------------------------------------------------------------------------------
// The scripts
// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

/// A string inside AppleScript double quotes.
fn applescript_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' | '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn outlook_script(path: &str, compose: &Compose) -> String {
    let recipients: String = compose
        .to
        .iter()
        .map(|a| format!("\tmake new recipient at msg with properties {{email address:{{address:{}}}}}\n", applescript_string(a.trim())))
        .collect();
    format!(
        "tell application \"Microsoft Outlook\"\n\
         \tset msg to make new outgoing message with properties {{subject:{s}, content:{b}}}\n\
         {recipients}\
         \tmake new attachment at msg with properties {{file:POSIX file {p}}}\n\
         \topen msg\n\
         \tactivate\n\
         end tell\n",
        s = applescript_string(&compose.subject),
        b = applescript_string(&compose.body),
        p = applescript_string(path),
    )
}

fn mail_app_script(path: &str, compose: &Compose) -> String {
    let recipients: String = compose
        .to
        .iter()
        .map(|a| {
            format!("\ttell msg to make new to recipient at end of to recipients with properties {{address:{}}}\n", applescript_string(a.trim()))
        })
        .collect();
    format!(
        "tell application \"Mail\"\n\
         \tset msg to make new outgoing message with properties {{subject:{s}, content:{b}, visible:true}}\n\
         {recipients}\
         \ttell msg to make new attachment with properties {{file name:POSIX file {p}}} at after the last paragraph\n\
         \tactivate\n\
         end tell\n",
        s = applescript_string(&compose.subject),
        b = applescript_string(&compose.body),
        p = applescript_string(path),
    )
}

/// The Simple MAPI script with `compose`'s values. Every value goes in as base64 of its UTF-8, so
/// no text (quotes of any kind, PowerShell treats ‘ ’ as quotes too) can end a string early.
/// `caller` is this program's process: its own windows (whose title can carry the subject too)
/// never count as the compose window.
fn mapi_script(compose: &Compose, caller: u32) -> String {
    let path = compose.path.to_string_lossy();
    let to = compose.to.iter().map(|a| a.trim()).collect::<Vec<_>>().join("\n");
    let b64 = |t: &str| format!("'{}'", base64(t.as_bytes()));
    fill(
        MAPI_SCRIPT,
        &[
            ("PATH", b64(&path)),
            ("NAME", b64(&file_name(&compose.path))),
            ("SUBJECT", b64(&compose.subject)),
            ("BODY", b64(&compose.body)),
            ("TO", b64(&to)),
            ("CALLER", caller.to_string()),
            ("SECONDS", MAPI_SECONDS.to_string()),
        ],
    )
}

/// `template` with each `@@KEY@@` replaced by its value, in one pass, so a value that happens to
/// contain `@@…@@` is never filled again. An unknown key is left as it is.
fn fill(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 256);
    let mut rest = template;
    while let Some(start) = rest.find("@@") {
        let (before, after) = rest.split_at(start);
        out.push_str(before);
        let tail = after.get(2..).unwrap_or("");
        let found = tail.find("@@").and_then(|end| {
            let key = tail.get(..end)?;
            values.iter().find(|(k, _)| *k == key).map(|(_, value)| (value, end))
        });
        match found {
            Some((value, end)) => {
                out.push_str(value);
                rest = tail.get(end + 2..).unwrap_or("");
            }
            None => {
                out.push_str("@@");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// What runs on Windows for a Simple MAPI client. It prints one answer line: `opened`, `hung`,
/// `code <MAPI result>` or `failed <why>`. The MAPI part is C# compiled by PowerShell itself
/// (`Add-Type`), which every Windows has: `MAPISendMailW` runs on a thread of its own while the
/// script looks for a new window, not this program's, whose title carries the subject (Outlook's
/// "<subject> - Message", Thunderbird's "Write: <subject>"). A client that answers without a
/// window (an error) is reported by its code.
const MAPI_SCRIPT: &str = r##"$ErrorActionPreference = 'Stop'
trap { 'failed ' + $_.Exception.Message; exit 0 }
$utf8 = New-Object System.Text.UTF8Encoding($false)
$text = { param($b) $utf8.GetString([Convert]::FromBase64String($b)) }
$path = & $text @@PATH@@; $name = & $text @@NAME@@; $subject = & $text @@SUBJECT@@; $body = & $text @@BODY@@
$to = [string[]]@((& $text @@TO@@) -split "`n" | Where-Object { $_ -ne '' })
$caller = @@CALLER@@
$src = @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
public static class PdfCraftMapi {
	[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
	public struct MapiFileDesc { public uint ulReserved; public uint flFlags; public uint nPosition; public string lpszPathName; public string lpszFileName; public IntPtr lpFileType; }
	[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
	public struct MapiMessage { public uint ulReserved; public string lpszSubject; public string lpszNoteText; public string lpszMessageType; public string lpszDateReceived; public string lpszConversationID; public uint flFlags; public IntPtr lpOriginator; public uint nRecipCount; public IntPtr lpRecips; public uint nFileCount; public IntPtr lpFiles; }
	[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
	public struct MapiRecipDesc { public uint ulReserved; public uint ulRecipClass; public string lpszName; public string lpszAddress; public uint ulEIDSize; public IntPtr lpEntryID; }
	[DllImport("mapi32.dll", CharSet = CharSet.Unicode)]
	static extern uint MAPISendMailW(IntPtr session, IntPtr window, ref MapiMessage message, uint flags, uint reserved);
	delegate bool EnumProc(IntPtr window, IntPtr data);
	[DllImport("user32.dll")] static extern bool EnumWindows(EnumProc proc, IntPtr data);
	[DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
	[DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr window, StringBuilder text, int max);
	[DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
	static List<IntPtr> Titled(string title, uint caller) {
		var found = new List<IntPtr>();
		var self = (uint)System.Diagnostics.Process.GetCurrentProcess().Id;
		EnumWindows(delegate(IntPtr w, IntPtr d) {
			if (!IsWindowVisible(w)) return true;
			uint process;
			GetWindowThreadProcessId(w, out process);
			if (process == caller || process == self) return true;
			var text = new StringBuilder(512);
			GetWindowText(w, text, 512);
			if (text.ToString().IndexOf(title, StringComparison.OrdinalIgnoreCase) >= 0) found.Add(w);
			return true;
		}, IntPtr.Zero);
		return found;
	}
	static uint Send(string path, string name, string subject, string body, string[] to) {
		var file = new MapiFileDesc { nPosition = 0xFFFFFFFF, lpszPathName = path, lpszFileName = name };
		var size = Marshal.SizeOf(typeof(MapiFileDesc));
		var files = Marshal.AllocHGlobal(size);
		var rsize = Marshal.SizeOf(typeof(MapiRecipDesc));
		var recips = to.Length > 0 ? Marshal.AllocHGlobal(rsize * to.Length) : IntPtr.Zero;
		try {
			Marshal.StructureToPtr(file, files, false);
			for (int i = 0; i < to.Length; i++) {
				var r = new MapiRecipDesc { ulRecipClass = 1, lpszName = to[i], lpszAddress = "SMTP:" + to[i] };
				Marshal.StructureToPtr(r, new IntPtr(recips.ToInt64() + i * rsize), false);
			}
			var message = new MapiMessage { lpszSubject = subject, lpszNoteText = body, nFileCount = 1, lpFiles = files, nRecipCount = (uint)to.Length, lpRecips = recips };
			return MAPISendMailW(IntPtr.Zero, IntPtr.Zero, ref message, 0x00000008 | 0x00000001, 0);
		} finally { Marshal.FreeHGlobal(files); if (recips != IntPtr.Zero) Marshal.FreeHGlobal(recips); }
	}
	public static void Run(string path, string name, string subject, string body, string[] to, uint caller, int seconds) {
		var before = Titled(subject, caller);
		uint result = 0;
		var worker = new Thread(delegate() { result = Send(path, name, subject, body, to); });
		worker.SetApartmentState(ApartmentState.STA);
		worker.IsBackground = true;
		worker.Start();
		var until = DateTime.UtcNow.AddSeconds(seconds);
		bool told = false;
		while (!worker.Join(250)) {
			if (told) continue;
			foreach (var w in Titled(subject, caller)) { if (!before.Contains(w)) { told = true; break; } }
			if (told) { Console.WriteLine("opened"); Console.Out.Flush(); }
			else if (DateTime.UtcNow > until) { Console.WriteLine("hung"); Console.Out.Flush(); return; }
		}
		if (!told) { Console.WriteLine("code " + result); Console.Out.Flush(); }
	}
}
'@
Add-Type -TypeDefinition $src -Language CSharp
[PdfCraftMapi]::Run($path, $name, $subject, $body, $to, [uint32]$caller, @@SECONDS@@)
exit 0
"##;

/// PowerShell's arguments for a script: no profile, no window, no prompt, the script encoded.
fn powershell_args(script: &str) -> Vec<String> {
    ["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-EncodedCommand"]
        .iter()
        .map(|a| a.to_string())
        .chain(std::iter::once(encode_powershell(script)))
        .collect()
}

/// `-EncodedCommand` takes the script as base64 of its UTF-16LE bytes.
fn encode_powershell(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                // Masked to six bits, so always inside the 64-entry table.
                let index = ((n >> (18 - 6 * i)) & 63) as usize;
                out.push(char::from(TABLE.get(index).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Base64 in lines of 76 characters, as MIME wants it.
fn base64_lines(bytes: &[u8]) -> String {
    let encoded = base64(bytes);
    // Base64 is ASCII, so every 76-byte cut is a character boundary.
    encoded.as_bytes().chunks(76).map(|line| String::from_utf8_lossy(line).into_owned()).collect::<Vec<_>>().join("\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compose() -> Compose {
        Compose { path: PathBuf::from("/tmp/Invoice \"26340\".pdf"), subject: "Invoice 26340".into(), body: String::new(), to: Vec::new() }
    }

    fn run_of(plan: &Plan) -> (&Path, &[String], Option<&str>, Watch) {
        match &plan.route {
            Route::Run { command, args, stdin, watch } => (command.as_path(), args.as_slice(), stdin.as_deref(), *watch),
            other => panic!("not a run: {other:?}"),
        }
    }

    fn fallback_of(plan: &Plan) -> (&Why, &Fallback) {
        match &plan.route {
            Route::Fallback { why, fallback } => (why, fallback),
            other => panic!("not the fallback: {other:?}"),
        }
    }

    #[test]
    fn macos_scripts_mail_and_outlook_and_falls_back_for_any_other_default() {
        let p = plan_for("macos", &DefaultMail::Outlook, &compose()).unwrap();
        assert_eq!(p.program.as_deref(), Some("Microsoft Outlook"));
        let (command, args, script, watch) = run_of(&p);
        assert_eq!((command, args, watch), (Path::new("/usr/bin/osascript"), &["-".to_string()][..], Watch::Exit));
        let script = script.unwrap();
        assert!(script.starts_with("tell application \"Microsoft Outlook\""));
        assert!(script.contains("POSIX file \"/tmp/Invoice \\\"26340\\\".pdf\""), "quotes escaped: {script}");
        assert!(script.contains("subject:\"Invoice 26340\""));

        let p = plan_for("macos", &DefaultMail::AppleMail, &compose()).unwrap();
        assert_eq!(p.program.as_deref(), Some("Mail"));
        assert!(run_of(&p).2.unwrap().starts_with("tell application \"Mail\""));

        // A browser (a webmail site) or any other program: the message without the file, and the file shown.
        let p = plan_for("macos", &DefaultMail::Other(Some("Firefox".into())), &compose()).unwrap();
        assert_eq!(p.program.as_deref(), Some("Firefox"));
        let (why, fallback) = fallback_of(&p);
        assert_eq!(why, &Why::CantAttach);
        assert_eq!(fallback.mailto, "mailto:?subject=Invoice%2026340");
        assert_eq!(fallback.reveal, compose().path);
        let p = plan_for("macos", &DefaultMail::Nothing, &compose()).unwrap();
        assert_eq!(fallback_of(&p).0, &Why::NoProgram);
        // Another system's default can't be used here.
        assert_eq!(fallback_of(&plan_for("macos", &DefaultMail::Mapi("Mozilla Thunderbird".into()), &compose()).unwrap()).0, &Why::CantAttach);
    }

    #[test]
    fn the_macos_default_is_read_from_launch_services_answer() {
        assert_eq!(macos_default("id=com.apple.mail\nname=Mail\npath=/System/Applications/Mail.app\n"), DefaultMail::AppleMail);
        assert_eq!(macos_default("id=com.microsoft.Outlook\nname=Microsoft Outlook\npath=/Applications/Microsoft Outlook.app"), DefaultMail::Outlook);
        assert_eq!(macos_default("id=org.mozilla.firefox\nname=Firefox\npath=/Applications/Firefox.app"), DefaultMail::Other(Some("Firefox".into())));
        assert_eq!(macos_default("id=com.example.mail\nname=\npath=/Applications/X.app"), DefaultMail::Other(Some("com.example.mail".into())));
        assert_eq!(macos_default("\n"), DefaultMail::Nothing);
        assert_eq!(macos_default(""), DefaultMail::Nothing);
    }

    #[test]
    fn windows_uses_the_new_outlook_then_simple_mapi_then_falls_back() {
        let olk = PathBuf::from("C:\\Users\\a\\AppData\\Local\\Microsoft\\WindowsApps\\olk.exe");
        let p = plan_for("windows", &DefaultMail::NewOutlook(olk.clone()), &compose()).unwrap();
        match &p.route {
            Route::MessageFile { program, message } => {
                assert_eq!(program, &olk);
                assert!(message.ends_with("Invoice \"26340\".eml") && message.starts_with(std::env::temp_dir()), "{}", message.display());
            }
            other => panic!("{other:?}"),
        }

        let p = plan_for("windows", &DefaultMail::Mapi("Mozilla Thunderbird".into()), &compose()).unwrap();
        assert_eq!(p.program.as_deref(), Some("Mozilla Thunderbird"));
        let (command, args, stdin, watch) = run_of(&p);
        assert!(command.ends_with("powershell.exe") && stdin.is_none() && watch == Watch::Answers);
        assert_eq!(args[args.len() - 2], "-EncodedCommand");
        let encoded = &args[args.len() - 1];
        assert!(encoded.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='));
        // Well under Windows' 32 767-character command line.
        assert!(encoded.len() < 24_000, "{} characters", encoded.len());

        assert_eq!(fallback_of(&plan_for("windows", &DefaultMail::Other(None), &compose()).unwrap()).0, &Why::CantAttach);
        assert_eq!(fallback_of(&plan_for("windows", &DefaultMail::Nothing, &compose()).unwrap()).0, &Why::NoProgram);
    }

    #[test]
    fn the_windows_default_is_read_from_the_registry() {
        let olk = Some(PathBuf::from("C:\\olk.exe"));
        let packaged = "HKEY_CLASSES_ROOT\\AppXabc\\Application\n    AppUserModelID    REG_SZ    Microsoft.OutlookForWindows_8wekyb3d8bbwe!Microsoft.OutlookforWindows\n";
        let new_outlook = DefaultMail::NewOutlook(PathBuf::from("C:\\olk.exe"));
        assert_eq!(windows_default(Some("AppXabc"), Some(packaged), Some("Microsoft Outlook".into()), olk.clone()), new_outlook);
        let desktop = "    (Default)    REG_SZ    \"C:\\Program Files\\WindowsApps\\olk.exe\" %1";
        assert_eq!(windows_default(Some("olk.mailto"), Some(desktop), None, olk.clone()), new_outlook);
        // Classic Outlook's command names OUTLOOK.EXE, which is not olk.exe.
        let classic = "    (Default)    REG_SZ    \"C:\\Program Files\\Microsoft Office\\root\\Office16\\OUTLOOK.EXE\" -c IPM.Note /m \"%1\"";
        assert_eq!(
            windows_default(Some("Outlook.URL.mailto.15"), Some(classic), Some("Microsoft Outlook".into()), olk.clone()),
            DefaultMail::Mapi("Microsoft Outlook".into())
        );
        // The new Outlook named but not installed: the MAPI client, or the fallback.
        assert_eq!(windows_default(Some("AppXabc"), Some(packaged), None, None), DefaultMail::Other(None));
        // A browser for webmail, and a MAPI client set up: the MAPI client takes the file.
        assert_eq!(
            windows_default(Some("ChromeHTML"), None, Some("Mozilla Thunderbird".into()), olk.clone()),
            DefaultMail::Mapi("Mozilla Thunderbird".into())
        );
        assert_eq!(windows_default(Some("ChromeHTML"), None, None, olk.clone()), DefaultMail::Other(None));
        assert_eq!(windows_default(None, None, None, olk), DefaultMail::Nothing);

        let out = "\r\nHKEY_LOCAL_MACHINE\\Software\\Clients\\Mail\r\n    (Predeterminado)    REG_SZ    Mozilla Thunderbird\r\n\r\n";
        assert_eq!(reg_value(out).as_deref(), Some("Mozilla Thunderbird"));
        assert_eq!(reg_value("HKEY_CURRENT_USER\\x\r\n    ProgId    REG_SZ    \r\n"), None);
        assert_eq!(reg_value("ERROR: The system was unable to find the specified registry key or value."), None);
    }

    #[test]
    fn the_mapi_script_carries_every_value_as_base64() {
        let mut c = compose();
        c.path = PathBuf::from("C:\\Temp\\Núñez’s 'x'.pdf");
        c.subject = "Núñez’s 'invoice'".into();
        c.body = "Hola\nadiós".into();
        c.to = vec!["ap@example.org".into(), "b@example.org".into()];
        let script = mapi_script(&c, 4242);
        assert!(!script.contains("@@"), "every value filled: {script}");
        assert!(!script.contains("Núñez") && !script.contains("invoice"), "no text goes in as text: {script}");
        assert!(script.contains(&format!("$subject = & $text '{}'", base64("Núñez’s 'invoice'".as_bytes()))));
        assert!(script.contains(&format!("$name = & $text '{}'", base64("Núñez’s 'x'.pdf".as_bytes()))));
        assert!(script.contains(&format!("(& $text '{}')", base64(b"ap@example.org\nb@example.org"))));
        assert!(script.contains("$caller = 4242") && script.contains(&format!("[uint32]$caller, {MAPI_SECONDS})")));
        assert!(script.contains("MAPISendMailW"));
        let bytes: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(base64(&bytes), encode_powershell(&script));
    }

    #[test]
    fn the_mapi_scripts_answers_are_read() {
        assert_eq!(mapi_answer("opened"), Some(None));
        assert_eq!(mapi_answer("hung"), Some(Some(Why::NoAnswer)));
        assert_eq!(mapi_answer("code 0"), Some(None));
        assert_eq!(mapi_answer(" code 1 \r"), Some(None), "closed by the person: shown, so not a failure");
        assert_eq!(mapi_answer("code 3"), Some(Some(Why::LogonFailed)));
        assert!(matches!(mapi_answer("code 2"), Some(Some(Why::Failed(_)))));
        assert_eq!(mapi_answer("failed Access denied"), Some(Some(Why::Failed("Access denied".into()))));
        assert_eq!(mapi_answer("code x"), None);
        assert_eq!(mapi_answer("WARNING: something"), None);
    }

    #[test]
    fn the_new_outlook_gets_an_unsent_message_with_the_file_inside() {
        let mut c = compose();
        c.subject = "Factura de Núñez".into();
        c.body = "Adjunta.".into();
        c.to = vec!["ap@example.org".into()];
        let message = unsent_message(&c, "Núñez.pdf", &[0u8; 200]);
        let lines: Vec<&str> = message.split("\r\n").collect();
        assert_eq!(lines[0], "To: ap@example.org");
        assert_eq!(lines[1], format!("Subject: =?UTF-8?B?{}?=", base64("Factura de Núñez".as_bytes())));
        assert_eq!(lines[2], "X-Unsent: 1");
        assert!(message.contains("Content-Type: application/pdf; name=\"=?UTF-8?B?"));
        assert!(message.contains("filename*=UTF-8''N%C3%BA%C3%B1ez.pdf"));
        assert!(message.contains(&base64(b"Adjunta.")));
        // The file in lines of 76 characters.
        assert!(lines.iter().filter(|l| l.starts_with("AAAA")).all(|l| l.len() <= 76));
        assert_eq!(lines.iter().filter(|l| l.starts_with("AAAA")).map(|l| l.len()).sum::<usize>(), base64(&[0u8; 200]).len());
        assert!(message.ends_with("--\r\n"));
        assert_eq!(message_name(Path::new("/x/a.b.pdf")), "a.b.eml");
        assert_eq!(message_name(Path::new("/")), "message.eml");
    }

    #[test]
    fn linux_and_the_bsds_use_xdg_email_and_fall_back_without_it() {
        let mut c = compose();
        c.to = vec!["ap@example.org".into(), "billing@example.org".into()];
        let xdg = DefaultMail::XdgEmail("/usr/bin/xdg-email".into());
        for os in ["linux", "freebsd", "openbsd"] {
            let p = plan_for(os, &xdg, &c).unwrap();
            let (command, args, stdin, watch) = run_of(&p);
            assert_eq!((command, stdin, watch), (Path::new("/usr/bin/xdg-email"), None, Watch::Started));
            let attach = args.iter().position(|a| a == "--attach").unwrap();
            assert_eq!(args[attach + 1], "/tmp/Invoice \"26340\".pdf");
            assert_eq!(&args[args.len() - 2..], ["ap@example.org", "billing@example.org"]);
            assert_eq!(fallback_of(&plan_for(os, &DefaultMail::Nothing, &c).unwrap()).0, &Why::NoProgram);
        }
        let dir = std::env::temp_dir().join(format!("pdfcraft-mail-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("xdg-email"), b"#!/bin/sh\n").unwrap();
        let path = std::env::join_paths([PathBuf::from("/definitely/not/here"), dir.clone()]).unwrap();
        assert_eq!(find_program(&path, "xdg-email"), Some(dir.join("xdg-email")));
        assert_eq!(find_program(&path, "missing-tool"), None);
        // A relative folder on PATH is never searched.
        assert_eq!(find_program(std::ffi::OsStr::new("."), "Cargo.toml"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_web_build_and_unknown_systems_are_unsupported() {
        assert_eq!(plan_for("unknown", &DefaultMail::Nothing, &compose()), Err(MailError::Unsupported));
        assert_eq!(plan_for("", &DefaultMail::AppleMail, &compose()), Err(MailError::Unsupported));
    }

    #[test]
    fn a_missing_file_or_a_bad_recipient_is_refused_before_anything_runs() {
        let woke = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = woke.clone();
        let c = Compose { path: PathBuf::from("/definitely/not/here.pdf"), subject: String::new(), body: String::new(), to: Vec::new() };
        assert!(matches!(send(c, move || flag.store(true, std::sync::atomic::Ordering::SeqCst)), Err(MailError::NoFile(_))));
        assert!(!woke.load(std::sync::atomic::Ordering::SeqCst), "nothing started");

        let file = std::env::temp_dir().join(format!("pdfcraft-mail-check-{}.pdf", std::process::id()));
        std::fs::write(&file, b"%PDF-1.7\n").unwrap();
        for bad in ["", "  ", "--attach", "a@example.org\r\nBcc: b@example.org", "a\u{0}@example.org"] {
            let c = Compose { path: file.clone(), subject: String::new(), body: String::new(), to: vec![bad.into()] };
            assert_eq!(send(c, || {}).err(), Some(MailError::BadRecipient(bad.into())), "{bad:?}");
        }
        let c = Compose { path: file.clone(), subject: String::new(), body: String::new(), to: vec!["a@example.org".into()] };
        assert_eq!(check(&c), Ok(()));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn mailto_addresses_encode_everything_but_the_plain_characters() {
        assert_eq!(mailto(&[], "", ""), "mailto:");
        assert_eq!(
            mailto(&["ap@example.org".into(), " b+c@example.org ".into()], "Q3, final", ""),
            "mailto:ap@example.org,b%2Bc@example.org?subject=Q3%2C%20final"
        );
        assert_eq!(mailto(&[], "Año ñ & = ?", "a\nb"), "mailto:?subject=A%C3%B1o%20%C3%B1%20%26%20%3D%20%3F&body=a%0D%0Ab");
        assert_eq!(mailto(&[], "", "x\r\ny\rz"), "mailto:?body=x%0D%0Ay%0D%0Az");
        // A name that reads like a parameter stays text.
        assert_eq!(mailto(&[], "x&attach=/etc/passwd", ""), "mailto:?subject=x%26attach%3D%2Fetc%2Fpasswd");
    }

    #[test]
    fn recipients_reach_every_system() {
        let mut c = compose();
        c.to = vec!["ap@example.org".into(), "billing@example.org".into()];
        let mac = plan_for("macos", &DefaultMail::AppleMail, &c).unwrap();
        assert!(run_of(&mac).2.unwrap().contains("make new to recipient at end of to recipients with properties {address:\"ap@example.org\"}"));
        let outlook = plan_for("macos", &DefaultMail::Outlook, &c).unwrap();
        assert!(run_of(&outlook).2.unwrap().contains("email address:{address:\"billing@example.org\"}"));
        let plan = plan_for("windows", &DefaultMail::Nothing, &c).unwrap();
        let (_, fallback) = fallback_of(&plan);
        assert!(fallback.mailto.starts_with("mailto:ap@example.org,billing@example.org?"));
    }

    #[test]
    fn filling_the_template_is_one_pass() {
        let filled = fill("a @@X@@ b @@Y@@ c @@Z@@", &[("X", "@@Y@@".into()), ("Y", "y".into())]);
        assert_eq!(filled, "a @@Y@@ b y c @@Z@@");
        assert_eq!(fill("@@", &[]), "@@");
        assert_eq!(fill("x@@X", &[("X", "1".into())]), "x@@X");
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_lines(&[0u8; 120]).split("\r\n").map(str::len).collect::<Vec<_>>(), [76, 76, 8]);
    }

    /// The helper's exit, or its answer line, decides; an early failure is reported in its own
    /// words. (Shell commands stand in for osascript, xdg-email and the MAPI script.)
    #[cfg(unix)]
    #[test]
    fn a_helpers_exit_or_answer_decides_and_failures_are_reported() {
        let sh = |script: &str| vec!["-c".to_string(), script.to_string()];
        let shell = Path::new("/bin/sh");
        assert_eq!(run(shell, &sh("exit 0"), None, Watch::Exit), None);
        assert_eq!(
            run(shell, &sh("echo noise >&2; echo 'Mail got an error: denied' >&2; exit 1"), None, Watch::Exit),
            Some(Why::Failed("Mail got an error: denied".into()))
        );
        // The script goes in on standard input.
        assert_eq!(run(shell, &[], Some("exit 0\n"), Watch::Exit), None);
        assert_eq!(run(shell, &[], Some("exit 3\n"), Watch::Exit), Some(Why::Failed("it stopped with an error".into())));
        assert!(matches!(run(shell, &sh("exit 4"), None, Watch::Started), Some(Why::Failed(_))));
        assert_eq!(run(shell, &sh("exit 0"), None, Watch::Started), None);
        assert_eq!(run(shell, &sh("echo WARNING: x; echo 'code 3'"), None, Watch::Answers), Some(Why::LogonFailed));
        assert_eq!(run(shell, &sh("echo opened; sleep 1"), None, Watch::Answers), None);
        assert_eq!(run(shell, &sh("echo nothing"), None, Watch::Answers), Some(Why::Failed("the email helper stopped without an answer".into())));
        assert!(matches!(run(Path::new("/definitely/not/here"), &[], None, Watch::Exit), Some(Why::Failed(_))));

        // The fallback runs nothing: it comes back for the caller to offer.
        let c = compose();
        let plan = plan_for("linux", &DefaultMail::Nothing, &c).unwrap();
        assert_eq!(execute(plan, &c), Answer::NotAttached { program: None, why: Why::NoProgram, fallback: fallback(&c) });
        let plan =
            Plan { program: Some("Mail".into()), route: Route::Run { command: shell.into(), args: sh("exit 1"), stdin: None, watch: Watch::Exit } };
        assert!(matches!(execute(plan, &c), Answer::NotAttached { program: Some(_), why: Why::Failed(_), .. }));
    }

    /// The AppleScript for Mail compiles (`osacompile` reads Mail's dictionary without starting it).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_mail_script_compiles() {
        use std::io::Write;
        let mut c = compose();
        c.to = vec!["ap@example.org".into()];
        c.body = "Line one\nLine \"two\"".into();
        let plan = plan_for("macos", &DefaultMail::AppleMail, &c).unwrap();
        let script = run_of(&plan).2.unwrap().to_string();
        let out = std::env::temp_dir().join(format!("pdfcraft-mail-{}.scpt", std::process::id()));
        let mut child = Command::new("/usr/bin/osacompile").arg("-o").arg(&out).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
        let done = child.wait_with_output().unwrap();
        let _ = std::fs::remove_file(&out);
        assert!(done.status.success(), "{script}\n{}", String::from_utf8_lossy(&done.stderr));
    }

    /// Finding the default runs the system's own tools and never fails: whatever it finds (or
    /// not) is a default. `--no-capture` prints it.
    #[test]
    fn this_computers_default_is_found_without_failing() {
        let found = detect();
        println!("default email program here: {found:?}");
        let c = compose();
        let os = std::env::consts::OS;
        if matches!(os, "macos" | "windows" | "linux" | "freebsd" | "openbsd" | "netbsd" | "dragonfly") {
            println!("plan here: {:?}", plan_for(os, &found, &c).map(|p| p.program));
        }
    }
}
