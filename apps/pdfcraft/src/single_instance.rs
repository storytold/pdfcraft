//! One PdfCraft per Windows sign-in or Linux/BSD display: a launch that only names files hands them
//! to the PdfCraft that is already running, where they open as tabs, and exits (#282, #317, #1726).
//!
//! Outlook attachments, Explorer double-clicks and Open With all start `pdfcraft.exe "<file>"`, and
//! a file manager or `xdg-open` starts `pdfcraft <file>` the same way, so until now every file got
//! its own process and window. The first PdfCraft listens on a Unix domain socket. On Windows (10
//! 1803 and later) it is in `%LOCALAPPDATA%\PdfCraft`, or in the data folder of a portable copy,
//! and is named after the Windows session (`instance-Console.sock`), so two sign-ins of the same
//! user never open files in each other's windows. On Linux and the BSDs it is in
//! `$XDG_RUNTIME_DIR/pdfcraft` and is named after the display (`instance-wayland-0.sock`), so a
//! launch on another display or seat opens its own window there. Either way it lives in a folder
//! only the user can reach, so other accounts can't create, replace or reach it: nobody else can
//! intercept a launch or open files in this user's window. Inside a Flatpak, `$XDG_RUNTIME_DIR` is
//! the app's own and is shared by every run of it, so the handoff works there too. The exchange is
//! one line each way, a JSON request naming absolute paths and `ok`, and the running app opens the
//! paths exactly as File ▸ Open would.
//!
//! Every failure falls back to the old behaviour: a launch that can't hand its files over within
//! a few seconds opens its own window. `--new-window`, any other option, or no files at all also
//! starts a window of its own, as before. macOS keeps its current behaviour (it already routes
//! Finder opens to the running app through Apple events).
//!
//! On Windows the socket comes from `uds_windows`, already in the lockfile through zbus: `std` has
//! no Unix sockets on Windows, and calling Winsock directly would need `unsafe`, which the
//! workspace forbids.

use std::sync::{Arc, Mutex, PoisonError};
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
use std::time::Duration;

#[cfg(all(unix, not(target_os = "macos")))]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

use pdfcraft_ui_egui::{OsEvent, OsEventsFn};

/// Longest request read: one line of JSON, far longer than any real list of paths.
#[cfg_attr(not(any(windows, all(unix, not(target_os = "macos")))), allow(dead_code))]
const MAX_REQUEST: u64 = 1 << 20;

/// How long either side waits for the other before giving up.
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
const TIMEOUT: Duration = Duration::from_secs(5);

/// What this launch should do.
#[cfg_attr(not(any(windows, all(unix, not(target_os = "macos")))), allow(dead_code))]
pub enum Claim {
    /// The running PdfCraft took the files: exit without a window.
    HandedOff,
    /// This is the first PdfCraft: open a window and accept later launches' files.
    Primary(Server),
    /// Open a window of its own and accept nothing (the behaviour before #282).
    Alone,
}

/// What a launch asks the running PdfCraft to do with its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Open them as tabs (a double-click, Open With, the "Edit with" entry).
    Open(Vec<String>),
    /// Add them to the Combine files list, in this order (the file manager's "Combine files" entry,
    /// #585). Nothing is combined until Combine is pressed.
    Combine(Vec<String>),
}

/// What later launches handed over, in arrival order, and how to wake the UI for it.
#[derive(Default)]
struct Inbox {
    requests: Vec<Request>,
    wake: Option<Box<dyn Fn() + Send>>,
}

/// The first PdfCraft's end of the socket. A background thread accepts launches until the
/// process exits; the UI picks their files up through [`Server::connect`].
pub struct Server {
    inbox: Arc<Mutex<Inbox>>,
}

impl Server {
    #[cfg_attr(not(any(windows, all(unix, not(target_os = "macos")))), allow(dead_code))]
    fn new() -> Self {
        Self { inbox: Arc::default() }
    }

    /// Queue another launch's request and wake the UI.
    #[cfg_attr(not(any(windows, all(unix, not(target_os = "macos")))), allow(dead_code))]
    fn deliver(&self, request: Request) {
        let mut inbox = self.inbox.lock().unwrap_or_else(PoisonError::into_inner);
        inbox.requests.push(request);
        if let Some(wake) = &inbox.wake {
            wake();
        }
    }

    /// The queue the UI drains every frame (`PdfCraftApp::os_events`). Files that arrived before
    /// the window existed are in the first batch. A batch also brings the window forward, as the
    /// new window it replaces would have been. A Wayland window can't bring itself forward (winit
    /// ignores `Focus` there), so on `native_wayland` it asks for the user's attention instead,
    /// which desktops show by highlighting it in the taskbar.
    pub fn connect(&self, ctx: &egui::Context, native_wayland: bool) -> OsEventsFn {
        let wake = ctx.clone();
        self.inbox.lock().unwrap_or_else(PoisonError::into_inner).wake = Some(Box::new(move || wake.request_repaint()));
        let inbox = Arc::clone(&self.inbox);
        let ctx = ctx.clone();
        Box::new(move || {
            let requests = std::mem::take(&mut inbox.lock().unwrap_or_else(PoisonError::into_inner).requests);
            if requests.is_empty() {
                return Vec::new();
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            if native_wayland {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
            }
            events(requests)
        })
    }
}

/// The window's events for a batch of requests, in arrival order: consecutive requests of one kind
/// become one event (files to open as tabs, or files for the Combine files list).
fn events(requests: Vec<Request>) -> Vec<OsEvent> {
    let mut out: Vec<OsEvent> = Vec::new();
    for request in requests {
        match (request, out.last_mut()) {
            (Request::Open(files), Some(OsEvent::Open(open))) => open.extend(files),
            (Request::Combine(files), Some(OsEvent::Combine(combine))) => combine.extend(files),
            (Request::Open(files), _) => out.push(OsEvent::Open(files)),
            (Request::Combine(files), _) => out.push(OsEvent::Combine(files)),
        }
    }
    out
}

/// The request a launch sends: `{"open":[…]}` or `{"combine":[…]}`, and a newline.
#[cfg_attr(not(any(windows, all(unix, not(target_os = "macos")))), allow(dead_code))]
pub fn request(request: &Request) -> String {
    let mut line = match request {
        Request::Open(paths) => serde_json::json!({ "open": paths }),
        Request::Combine(paths) => serde_json::json!({ "combine": paths }),
    }
    .to_string();
    line.push('\n');
    line
}

/// The request a line holds, or `None` when it isn't one: anything but one JSON object whose
/// `open` or `combine` is a non-empty list of non-empty strings. A PdfCraft from before Combine
/// files (#585) refuses a combine request, and that launch opens a window of its own instead.
#[cfg_attr(not(any(windows, all(unix, not(target_os = "macos")))), allow(dead_code))]
pub fn parse_request(line: &str) -> Option<Request> {
    let value: serde_json::Value = serde_json::from_str(line.trim_end()).ok()?;
    let (list, kind): (_, fn(Vec<String>) -> Request) = match (value.get("open"), value.get("combine")) {
        (Some(open), None) => (open, Request::Open),
        (None, Some(combine)) => (combine, Request::Combine),
        _ => return None,
    };
    let paths = list.as_array()?.iter().map(|p| p.as_str().filter(|s| !s.is_empty()).map(str::to_string)).collect::<Option<Vec<String>>>()?;
    (!paths.is_empty()).then(|| kind(paths))
}

/// File arguments made absolute against this launch's working folder, which the running app
/// doesn't share. A path that can't be made absolute is sent as it is.
pub fn absolute(paths: &[String]) -> Vec<String> {
    paths.iter().map(|p| std::path::absolute(p).map_or_else(|_| p.clone(), |a| a.to_string_lossy().into_owned())).collect()
}

/// Decide what this launch does. The request's files are absolute ([`absolute`]); `may_hand_off`
/// is false when the launch asked for anything besides its files, so it gets its own window.
pub fn claim(request: &Request, may_hand_off: bool) -> Claim {
    #[cfg(any(windows, all(unix, not(target_os = "macos"))))]
    {
        match socket_path() {
            Some(path) => claim_at(&path, request, may_hand_off),
            None => Claim::Alone,
        }
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        let _ = (request, may_hand_off);
        Claim::Alone
    }
}

/// The socket in `%LOCALAPPDATA%\PdfCraft` (beside the crash-recovery folder); none when
/// `%LOCALAPPDATA%` is unset.
#[cfg(windows)]
fn socket_path() -> Option<std::path::PathBuf> {
    let name = socket_name(session_id(), std::env::var("SESSIONNAME").ok().as_deref());
    // %LOCALAPPDATA% is private to the user's sign-in, so other accounts can't receive this
    // user's file paths or send it files to open. A portable copy uses it too (#889), under its
    // own name so it never hands files to an installed copy (which keeps its settings elsewhere).
    // Never the portable folder itself: it may sit where other accounts can write (`C:\Tools`).
    let name = if pdfcraft_ui_egui::portable::data_dir().is_some() { format!("portable-{name}") } else { name };
    std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()).map(|d| std::path::PathBuf::from(d).join("PdfCraft").join(name))
}

/// The socket in `$XDG_RUNTIME_DIR/pdfcraft`; none when `$XDG_RUNTIME_DIR` is unset. Never `/tmp`
/// instead: other accounts can create files there, and so could listen in this user's place.
#[cfg(all(unix, not(target_os = "macos")))]
fn socket_path() -> Option<std::path::PathBuf> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let name = display_socket_name(var("WAYLAND_DISPLAY").or_else(|| var("DISPLAY")).as_deref());
    // The XDG base directory spec makes it the user's own (mode 0700), so the folder inside it is
    // out of other accounts' reach whatever the umask.
    var("XDG_RUNTIME_DIR").map(|d| std::path::PathBuf::from(d).join("pdfcraft").join(name))
}

/// The socket's file name for a display (`WAYLAND_DISPLAY`, else `DISPLAY`): a launch opens its
/// files on the display it was started on, never in a window on another seat, a nested
/// compositor or a remote X display. Letters, digits, `-`, `_` and `.` are kept, and of those the
/// last 32, where an absolute `WAYLAND_DISPLAY` (`/run/user/1000/wayland-1`) differs.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn display_socket_name(display: Option<&str>) -> String {
    let kept: Vec<char> = display.unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    let display: String = kept.get(kept.len().saturating_sub(32)..).unwrap_or_default().iter().collect();
    if display.is_empty() { "instance.sock".to_string() } else { format!("instance-{display}.sock") }
}

/// The Windows session this process runs in, from its access token: the console, each Remote
/// Desktop sign-in and session 0 (services) each have their own. `None` if Windows won't say.
#[cfg(windows)]
fn session_id() -> Option<u32> {
    use winsafe::{HACCESSTOKEN, TokenInfo, co::TOKEN_INFORMATION_CLASS};
    match HACCESSTOKEN::GetCurrentProcessToken().GetTokenInformation(TOKEN_INFORMATION_CLASS::SessionId) {
        Ok(TokenInfo::SessionId(id)) => Some(*id),
        _ => None,
    }
}

/// The socket's file name for a Windows session. The same user signed in twice (at the console
/// and over Remote Desktop) gets one socket per session, so a file never opens in a window on the
/// other screen. The session is the token's session id (`session`): a launch from Task Scheduler,
/// a service or a remote shell has no `SESSIONNAME` but still runs in a session, so it finds the
/// window of the session it runs in (#3338), and never one in another session (review on #3340).
/// Only if the id can't be read does `SESSIONNAME` (`Console`, `RDP-Tcp#3`…) name it, letters,
/// digits and `-` kept.
#[cfg_attr(not(windows), allow(dead_code))]
fn socket_name(session: Option<u32>, session_name: Option<&str>) -> String {
    if let Some(id) = session {
        return format!("instance-s{id}.sock");
    }
    let name: String = session_name.unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(32).collect();
    if name.is_empty() { "instance.sock".to_string() } else { format!("instance-{name}.sock") }
}

/// Held while a launch finds out whether it is the first, so launches that start together (several
/// files selected in Explorer and opened at once) take turns: the first one is listening before
/// the next one looks. Without it a launch could find the socket bound but not yet listening,
/// fail to clear it ("Access is denied") and open a window of its own. The lock is on a file
/// beside the socket, which is never deleted; the system releases it when the process ends,
/// however it ends. Best effort: without the lock the launch goes on as before.
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
fn election(socket: &std::path::Path) -> Option<std::fs::File> {
    if let Some(dir) = socket.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(socket.with_extension("lock")).ok()?;
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        match file.try_lock() {
            Ok(()) => return Some(file),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                log::info!("single instance: no election lock ({e})");
                return None;
            }
        }
    }
}

/// [`claim`] with the socket at `path`.
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
fn claim_at(path: &std::path::Path, request: &Request, may_hand_off: bool) -> Claim {
    // Released when this function returns: by then this launch listens, or has handed over.
    let _election = election(path);
    // Two launches can start together: the one that loses the race to bind hands over to the
    // one that won. A third round is for a socket that vanishes in between.
    for _ in 0..3 {
        match UnixStream::connect(path) {
            Ok(stream) => {
                if !may_hand_off {
                    return Claim::Alone;
                }
                return match hand_off(&stream, request) {
                    Ok(()) => Claim::HandedOff,
                    Err(e) => {
                        log::warn!("couldn't hand the files to the running PdfCraft ({e}); opening a new window");
                        Claim::Alone
                    }
                };
            }
            // Nobody is listening: no socket yet, or one left by a PdfCraft that didn't exit
            // cleanly (connecting to it is refused). Clear it and become the first. Removed
            // without asking `path.exists()` first: on Windows that follows the socket's reparse
            // point, which can fail, and a leftover socket would then block every later start.
            Err(_) => match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                // Another launch has bound the socket and is about to listen: try it again.
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) => {
                    log::info!("single instance off: can't clear {} ({e})", path.display());
                    return Claim::Alone;
                }
            },
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match UnixListener::bind(path) {
            Ok(listener) => return serve(listener),
            // Another launch bound it first: hand over to that one on the next round.
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            // Older Windows, a path too long for a socket address, a file system without socket
            // support (some network or removable drives): this launch works as before.
            Err(e) => {
                log::info!("single instance off: can't listen at {} ({e})", path.display());
                return Claim::Alone;
            }
        }
    }
    Claim::Alone
}

/// Send `files` over `stream` and wait for the running app's `ok`.
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
fn hand_off(stream: &UnixStream, request: &Request) -> std::io::Result<()> {
    use std::io::{BufRead, Read, Write};
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut writer = stream;
    writer.write_all(self::request(request).as_bytes())?;
    let mut reply = String::new();
    std::io::BufReader::new(stream.take(16)).read_line(&mut reply)?;
    if reply == "ok\n" { Ok(()) } else { Err(std::io::Error::other("the running PdfCraft didn't confirm")) }
}

/// Accept launches on a background thread for as long as the process runs.
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
fn serve(listener: UnixListener) -> Claim {
    let server = Server::new();
    let inbox = Server { inbox: Arc::clone(&server.inbox) };
    let started = std::thread::Builder::new().name("single-instance".into()).spawn(move || {
        for stream in listener.incoming() {
            let result = stream.and_then(|s| receive(&s, &inbox));
            if let Err(e) = result {
                log::warn!("single instance: a launch's request failed ({e})");
                // Don't spin if accepting keeps failing.
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    });
    match started {
        Ok(_) => Claim::Primary(server),
        Err(e) => {
            log::warn!("single instance off: no listener thread ({e})");
            Claim::Alone
        }
    }
}

/// Read one launch's request, queue its files and confirm.
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
fn receive(stream: &UnixStream, server: &Server) -> std::io::Result<()> {
    use std::io::{BufRead, Read, Write};
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut line = String::new();
    std::io::BufReader::new(stream.take(MAX_REQUEST)).read_line(&mut line)?;
    // A launch that wanted its own window only checked that this one is running.
    if line.is_empty() {
        return Ok(());
    }
    let request = parse_request(&line).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "not an open or combine request"))?;
    server.deliver(request);
    let mut writer = stream;
    writer.write_all(b"ok\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_names_the_paths_and_round_trips() {
        let paths = vec![r"C:\Users\me\Documents\Quote.pdf".to_string(), r"D:\Scans\Plan „A“ 1.pdf".to_string()];
        let line = request(&Request::Open(paths.clone()));
        assert!(line.ends_with('\n') && !line.trim_end().contains('\n'), "one line");
        assert_eq!(parse_request(&line), Some(Request::Open(paths)));
    }

    #[test]
    fn a_combine_request_names_its_files_and_round_trips() {
        let paths = vec![r"C:\Scans\Quote.pdf".to_string(), r"C:\Scans\Photo 1.jpg".to_string()];
        let line = request(&Request::Combine(paths.clone()));
        assert!(line.starts_with(r#"{"combine":"#) && line.ends_with('\n') && !line.trim_end().contains('\n'), "{line}");
        assert_eq!(parse_request(&line), Some(Request::Combine(paths)));
    }

    #[test]
    fn anything_but_a_list_of_paths_is_refused() {
        for line in [
            "",
            "ok\n",
            "{}",
            r#"{"open":[]}"#,
            r#"{"open":[""]}"#,
            r#"{"open":["a.pdf",3]}"#,
            r#"{"open":"a.pdf"}"#,
            "[\"a.pdf\"]",
            r#"{"combine":[]}"#,
            r#"{"combine":[""]}"#,
            r#"{"combine":"a.pdf"}"#,
            r#"{"open":["a.pdf"],"combine":["b.pdf"]}"#,
        ] {
            assert_eq!(parse_request(line), None, "{line:?}");
        }
    }

    #[test]
    fn relative_paths_are_made_absolute_against_this_launch() {
        let out = absolute(&["Quote.pdf".to_string()]);
        assert_eq!(out, vec![std::env::current_dir().unwrap().join("Quote.pdf").to_string_lossy().into_owned()]);
        let already = std::env::current_dir().unwrap().join("x.pdf").to_string_lossy().into_owned();
        assert_eq!(absolute(std::slice::from_ref(&already)), vec![already]);
    }

    #[test]
    fn each_windows_session_gets_its_own_socket_name() {
        // The session id decides: #3338, a launch from Task Scheduler (no SESSIONNAME) in the
        // console's session finds the window opened from Explorer there...
        assert_eq!(socket_name(Some(1), None), socket_name(Some(1), Some("Console")));
        // ...and (review on #3340) one in a Remote Desktop session never reaches the console's.
        assert_ne!(socket_name(Some(2), None), socket_name(Some(1), Some("Console")));
        assert_ne!(socket_name(Some(2), Some("RDP-Tcp#3")), socket_name(Some(3), Some("RDP-Tcp#3")));
        assert_eq!(socket_name(Some(4_000_000_000), None), "instance-s4000000000.sock");
        // Only without an id does SESSIONNAME name it.
        assert_eq!(socket_name(None, Some("Console")), "instance-Console.sock");
        assert_eq!(socket_name(None, Some("RDP-Tcp#3")), "instance-RDP-Tcp3.sock");
        assert_ne!(socket_name(None, Some("RDP-Tcp#3")), socket_name(None, Some("RDP-Tcp#4")));
        assert_eq!(socket_name(None, Some(r"..\x/y:z")), "instance-xyz.sock", "never a path");
        assert_eq!(socket_name(None, None), "instance.sock");
        assert_eq!(socket_name(None, Some("")), "instance.sock");
    }

    #[test]
    fn delivered_files_reach_the_ui_once_wake_it_and_bring_the_window_forward() {
        let server = Server::new();
        // Before the window exists: queued, nothing to wake yet.
        server.deliver(Request::Open(vec!["a.pdf".into()]));
        let ctx = egui::Context::default();
        let mut poll = server.connect(&ctx, false);
        server.deliver(Request::Open(vec!["b.pdf".into()]));
        let events = poll();
        assert!(matches!(events.as_slice(), [OsEvent::Open(p)] if p == &["a.pdf", "b.pdf"]));
        assert!(poll().is_empty(), "each file is delivered once");
    }

    #[test]
    fn each_display_gets_its_own_socket_name() {
        assert_eq!(display_socket_name(Some("wayland-0")), "instance-wayland-0.sock");
        assert_eq!(display_socket_name(Some(":0")), "instance-0.sock");
        assert_ne!(display_socket_name(Some(":1.0")), display_socket_name(Some(":10")));
        assert_ne!(display_socket_name(Some("wayland-0")), display_socket_name(Some("wayland-1")));
        // An absolute WAYLAND_DISPLAY keeps the end of its path, where nested compositors differ.
        let nested = display_socket_name(Some("/home/someone/.cache/nested-compositor/run/wayland-1"));
        assert!(nested.ends_with("wayland-1.sock") && !nested.contains('/'), "{nested}");
        assert_eq!(display_socket_name(Some("../../x/y")), "instance-....xy.sock", "never a path");
        assert_eq!(display_socket_name(None), "instance.sock");
        assert_eq!(display_socket_name(Some("")), "instance.sock");
    }

    /// The socket is named by the session the process runs in, which Windows always gives.
    #[cfg(windows)]
    #[test]
    fn this_process_has_a_session_id() {
        let id = super::session_id();
        assert!(id.is_some(), "the access token names the session");
        let name = socket_name(id, None);
        assert!(socket_path().is_none_or(|p| p.file_name().is_some_and(|f| f.to_string_lossy().ends_with(&name))), "{name}");
    }

    #[cfg(any(windows, all(unix, not(target_os = "macos"))))]
    fn socket(test: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-instance-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(socket_name(None, None))
    }

    #[cfg(any(windows, all(unix, not(target_os = "macos"))))]
    #[test]
    fn a_second_launch_hands_its_files_to_the_first() {
        let path = socket("hand-off");
        let Claim::Primary(first) = claim_at(&path, &Request::Open(Vec::new()), true) else { panic!("the first launch listens") };
        let mut poll = first.connect(&egui::Context::default(), false);
        assert!(matches!(claim_at(&path, &Request::Open(vec!["C:\\a.pdf".into()]), true), Claim::HandedOff));
        assert!(matches!(poll().as_slice(), [OsEvent::Open(p)] if p == &["C:\\a.pdf"]));
        // A launch with options keeps its own window and doesn't take over the socket.
        assert!(matches!(claim_at(&path, &Request::Open(vec!["C:\\b.pdf".into()]), false), Claim::Alone));
        assert!(poll().is_empty());
    }

    #[cfg(any(windows, all(unix, not(target_os = "macos"))))]
    #[test]
    fn launches_started_together_share_one_window() {
        // Several files selected in Explorer or a file manager and opened at once: one process per file.
        let path = socket("together");
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let launches: Vec<_> = (0..8)
            .map(|i| {
                let (path, barrier) = (path.clone(), Arc::clone(&barrier));
                std::thread::spawn(move || {
                    barrier.wait();
                    claim_at(&path, &Request::Open(vec![format!("C:\\{i}.pdf")]), true)
                })
            })
            .collect();
        // The first launch's listener lives until every other launch has handed over.
        let claims: Vec<Claim> = launches.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(claims.iter().filter(|c| matches!(c, Claim::Primary(_))).count(), 1, "one window");
        assert_eq!(claims.iter().filter(|c| matches!(c, Claim::HandedOff)).count(), 7, "the rest hand over");
    }

    #[cfg(any(windows, all(unix, not(target_os = "macos"))))]
    #[test]
    fn a_socket_left_by_a_crashed_instance_is_replaced() {
        let path = socket("stale");
        // Bind and drop: the file stays, nobody listens.
        drop(UnixListener::bind({
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            &path
        }));
        // `symlink_metadata`, not `exists`, which follows the socket's reparse point and can fail.
        assert!(std::fs::symlink_metadata(&path).is_ok(), "the socket file stays behind");
        assert!(matches!(claim_at(&path, &Request::Open(vec!["C:\\a.pdf".into()]), true), Claim::Primary(_)));
    }

    #[test]
    fn opened_and_combined_files_reach_the_ui_in_arrival_order() {
        let server = Server::new();
        let mut poll = server.connect(&egui::Context::default(), false);
        server.deliver(Request::Open(vec!["a.pdf".into()]));
        server.deliver(Request::Combine(vec!["b.pdf".into()]));
        server.deliver(Request::Combine(vec!["c.png".into()]));
        server.deliver(Request::Open(vec!["d.pdf".into()]));
        let events = poll();
        assert!(
            matches!(events.as_slice(), [OsEvent::Open(a), OsEvent::Combine(bc), OsEvent::Open(d)] if a == &["a.pdf"] && bc == &["b.pdf", "c.png"] && d == &["d.pdf"]),
            "one event per run of one kind"
        );
        assert!(poll().is_empty());
    }

    /// The file manager's "Combine files" on a selection: one launch per selected file, PDFs and
    /// pictures mixed, all at once (#585). One becomes the window; every other hands its file over
    /// for the Combine files list, never as a tab.
    #[cfg(any(windows, all(unix, not(target_os = "macos"))))]
    #[test]
    fn combine_launches_started_together_reach_one_window() {
        let path = socket("combine");
        let names = ["1.pdf", "2.png", "3.pdf", "4.jpg", "5.tif", "6.pdf"];
        let barrier = Arc::new(std::sync::Barrier::new(names.len()));
        let launches: Vec<_> = names
            .iter()
            .map(|name| {
                let (path, barrier, file) = (path.clone(), Arc::clone(&barrier), name.to_string());
                std::thread::spawn(move || {
                    barrier.wait();
                    (file.clone(), claim_at(&path, &Request::Combine(vec![file]), true))
                })
            })
            .collect();
        let claims: Vec<(String, Claim)> = launches.into_iter().map(|t| t.join().unwrap()).collect();
        let window: Vec<&(String, Claim)> = claims.iter().filter(|(_, c)| matches!(c, Claim::Primary(_))).collect();
        assert_eq!(window.len(), 1, "one window");
        assert_eq!(claims.iter().filter(|(_, c)| matches!(c, Claim::HandedOff)).count(), names.len() - 1, "the rest hand over");
        let (own, Claim::Primary(server)) = window[0] else { panic!("the window's claim") };
        let events = server.connect(&egui::Context::default(), false)();
        let [OsEvent::Combine(handed)] = events.as_slice() else { panic!("one Combine event and no tabs, got {} events", events.len()) };
        // The window lists its own file itself; every other file arrived once.
        let mut handed = handed.clone();
        handed.sort();
        let mut expected: Vec<String> = names.iter().map(|n| n.to_string()).filter(|n| n != own).collect();
        expected.sort();
        assert_eq!(handed, expected);
    }
}
