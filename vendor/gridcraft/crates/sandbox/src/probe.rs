//! The enforcement check a confined launcher runs before it starts its program, and the fuller
//! report `gridcraft-cli sandbox-check` prints.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::PROBE_CHILD_FLAG;

/// What one attempt found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The attempt was refused by the system, as the sandbox requires.
    Denied,
    /// The attempt was not refused (it worked, or failed for another reason, which doesn't prove
    /// the sandbox); the text says what happened.
    Allowed(String),
}

/// One attempt and its outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub outcome: Outcome,
}

/// A loopback address with nothing to talk to: unconfined, connecting fails with "refused", which
/// is not a denial; confined, creating the socket or connecting is denied.
const TCP_TARGET: ([u8; 4], u16) = ([127, 0, 0, 1], 9);

/// Every attempt that must be denied, against `canary` (a file outside the policy), `exe` (the
/// current executable, which the process check tries to start) and `target` (from
/// [`signal_target`]). Each must report [`Outcome::Denied`] before the launcher runs its program.
pub fn enforcement(canary: &Path, exe: &Path, target: u32) -> Vec<Check> {
    vec![
        Check { name: "read the canary", outcome: denied(std::fs::read(canary).map(drop)) },
        Check { name: "change the canary's permissions", outcome: denied(chmod_probe(canary)) },
        Check { name: "truncate the canary", outcome: denied(truncate_probe(canary)) },
        Check { name: "write next to the canary", outcome: write_probe(canary.parent().unwrap_or(canary)) },
        Check { name: "write in the temporary directory", outcome: write_probe(&std::env::temp_dir()) },
        Check { name: "TCP connection", outcome: denied(tcp_probe()) },
        Check { name: "start a process", outcome: spawn_probe(exe) },
        Check { name: "signal another process", outcome: signal_probe(target) },
        #[cfg(target_os = "linux")]
        Check { name: "switch a descriptor's notifications on", outcome: async_probe() },
    ]
}

/// Asks for the permissions the file has, so nothing changes even where this is allowed.
fn chmod_probe(file: &Path) -> std::io::Result<()> {
    std::fs::set_permissions(file, std::fs::metadata(file)?.permissions())
}

/// Asks for the length the file has, so nothing changes even where this is allowed.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn truncate_probe(file: &Path) -> std::io::Result<()> {
    let length = nix::libc::off_t::try_from(std::fs::metadata(file)?.len()).map_err(|e| std::io::Error::new(ErrorKind::InvalidInput, e))?;
    nix::unistd::truncate(file, length).map_err(std::io::Error::from)
}

/// Other systems have no confinement, so no launch gets this far.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn truncate_probe(_: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(ErrorKind::Unsupported, "this system has no sandbox"))
}

/// Signal 0 to `process`, which delivers nothing: it only asks whether a signal would be allowed.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn signal_zero(process: u32) -> std::io::Result<()> {
    let pid = i32::try_from(process).map_err(|e| std::io::Error::new(ErrorKind::InvalidInput, e))?;
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).map_err(std::io::Error::from)
}

/// The process the check tries a signal on: this launcher's parent, if a signal to it is allowed
/// now, before any confinement. One that refuses already (PID 1, another user's process) would be
/// refused afterwards too, which would prove nothing, so the launch is refused instead.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn signal_target() -> std::result::Result<u32, String> {
    signalable(std::os::unix::process::parent_id())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn signalable(process: u32) -> std::result::Result<u32, String> {
    signal_zero(process)
        .map(|()| process)
        .map_err(|e| format!("a signal to process {process} fails before any confinement, so its refusal would prove nothing: {e}"))
}

/// Whether a signal to `target` is refused now. A parent that changed since [`signal_target`] looked
/// is another process, which may refuse for its own reasons (a launcher whose parent exited has PID 1
/// for a parent).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn signal_probe(target: u32) -> Outcome {
    if std::os::unix::process::parent_id() != target {
        return Outcome::Allowed(format!("this launcher's parent is no longer process {target}"));
    }
    denied(signal_zero(target))
}

/// Other systems have no confinement, so no launch gets this far.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn signal_target() -> std::result::Result<u32, String> {
    Err("this system has no sandbox".into())
}

/// Other systems have no confinement, so no launch gets this far.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn signal_probe(_: u32) -> Outcome {
    Outcome::Allowed("this system has no sandbox".into())
}

/// `Denied` when `r` failed with "permission denied" (`EACCES` or `EPERM`), else what happened.
fn denied(r: std::io::Result<()>) -> Outcome {
    match r {
        Ok(()) => Outcome::Allowed("it worked".into()),
        Err(e) if e.kind() == ErrorKind::PermissionDenied => Outcome::Denied,
        Err(e) => Outcome::Allowed(format!("it failed for another reason: {e}")),
    }
}

fn write_probe(dir: &Path) -> Outcome {
    let file = dir.join(format!("gridcraft-sandbox-write-probe-{}", std::process::id()));
    let r = std::fs::OpenOptions::new().write(true).create_new(true).open(&file);
    if r.is_ok() {
        let _ = std::fs::remove_file(&file);
    }
    denied(r.map(drop))
}

/// `O_ASYNC` on a descriptor of the program's own: the switch that has the kernel signal the
/// descriptor's owner when its state changes. The owner may be a process the program didn't name, a
/// standard stream the parent made, so on Linux the filter refuses setting it, the same way it
/// refuses the commands that name another owner. Naming one can't be asked here without `fcntl`'s
/// variable-argument form, which no safe API wraps; the rules refuse those commands, and the
/// confined-program test covers them.
#[cfg(target_os = "linux")]
fn async_probe() -> Outcome {
    let r = nix::unistd::pipe().map_err(std::io::Error::from).and_then(|(_, write)| {
        nix::fcntl::fcntl(write, nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_ASYNC)).map(drop).map_err(std::io::Error::from)
    });
    denied(r)
}

fn tcp_probe() -> std::io::Result<()> {
    TcpStream::connect_timeout(&SocketAddr::from(TCP_TARGET), Duration::from_secs(2)).map(drop)
}

fn spawn_probe(exe: &Path) -> Outcome {
    let child = Command::new(exe).arg(PROBE_CHILD_FLAG).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    match child {
        Ok(mut c) => {
            let _ = c.wait();
            Outcome::Allowed("a process started".into())
        }
        // A start refused by the sandbox: no new process (seccomp, or the macOS profile).
        Err(e) if e.kind() == ErrorKind::PermissionDenied => Outcome::Denied,
        Err(e) => Outcome::Allowed(format!("it failed for another reason: {e}")),
    }
}

/// `--sandbox-report <canary> [<readable file> [<home directory>]]`, run confined: prints one line per attempt,
/// `denied: <name>` or `allowed: <name> (<detail>)`, then the attempts that must work, and exits
/// with 0 when every line is as the sandbox requires, 1 otherwise. Used by
/// `gridcraft-cli sandbox-check` and the tests.
pub fn report(args: &[OsString]) -> i32 {
    let (Some(canary), readable) = (args.first(), args.get(1)) else {
        eprintln!("gridcraft-sandbox: --sandbox-report needs the canary's path");
        return 2;
    };
    let exe = std::env::current_exe().unwrap_or_default();
    let mut ok = true;
    // The launcher looked at this process's parent before it confined itself, and `exec` keeps it.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let target = std::os::unix::process::parent_id();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let target = 0;
    let mut checks = enforcement(Path::new(canary), &exe, target);
    checks.push(Check { name: "UDP socket", outcome: denied(UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0))).map(drop)) });
    // The person's own files: not part of the launch check, since a policy may allow a directory
    // under the home directory (a Python installed there), but no policy here allows the home
    // directory itself.
    // The confined process inherits no `HOME` (its environment is cleared), so the directory is
    // an argument.
    if let Some(home) = args.get(2).filter(|h| !h.is_empty()) {
        checks.push(Check { name: "list the home directory", outcome: denied(std::fs::read_dir(home).map(drop)) });
    }
    for c in &checks {
        match &c.outcome {
            Outcome::Denied => println!("denied: {}", c.name),
            Outcome::Allowed(detail) => {
                ok = false;
                println!("allowed: {} ({detail})", c.name);
            }
        }
    }
    // What a confined program still needs.
    let thread = std::thread::Builder::new().spawn(|| 2 + 2).map(|h| h.join().is_ok());
    ok &= worked("start a thread", thread.as_ref().map(|j| *j).unwrap_or(false), thread.err().map(|e| e.to_string()));
    if let Some(file) = readable {
        let r = std::fs::read(file);
        ok &= worked("read an allowed file", r.is_ok(), r.err().map(|e| e.to_string()));
    }
    i32::from(!ok)
}

fn worked(name: &str, ok: bool, err: Option<String>) -> bool {
    if ok {
        println!("works: {name}");
    } else {
        println!("broken: {name} ({})", err.unwrap_or_default());
    }
    ok
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn a_signal_that_fails_before_any_confinement_proves_nothing_after_it() {
        // This test isn't confined, and its parent has the same user.
        let parent = std::os::unix::process::parent_id();
        assert_eq!(signal_target(), Ok(parent));
        assert!(matches!(signal_probe(parent), Outcome::Allowed(_)), "allowed unconfined, so no denial");
        // No such process: there is nothing to compare a refusal with, so the launch is refused.
        for process in [u32::MAX, i32::MAX as u32] {
            let e = signalable(process).unwrap_err();
            assert!(e.contains("before any confinement"), "{e}");
        }
    }

    /// Unconfined, the notification probe reports what the system allows; confined, the launch
    /// check needs it `Denied`. Either way it comes out of the same three calls, so this proves
    /// the probe runs and classifies rather than failing outright.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_notification_probe_classifies() {
        assert!(matches!(async_probe(), Outcome::Allowed(_)), "unconfined the switch can be set");
    }

    #[test]
    fn the_checks_on_the_canary_change_nothing_where_they_are_allowed() {
        let canary = crate::Canary::create().unwrap();
        let file = canary.file();
        let state = || {
            let m = std::fs::metadata(file).unwrap();
            (std::fs::read(file).unwrap(), m.permissions(), m.len())
        };
        let before = state();
        // This test isn't confined, so each works, which isn't a denial.
        assert_eq!(denied(chmod_probe(file)), Outcome::Allowed("it worked".into()));
        assert_eq!(denied(truncate_probe(file)), Outcome::Allowed("it worked".into()));
        assert_eq!(state(), before);
    }

    #[test]
    fn a_missing_canary_proves_nothing() {
        let canary = crate::Canary::create().unwrap();
        let gone = canary.file().with_file_name("not there");
        for outcome in [denied(chmod_probe(&gone)), denied(truncate_probe(&gone))] {
            assert!(matches!(&outcome, Outcome::Allowed(detail) if detail.contains("another reason")), "{outcome:?}");
        }
    }

    #[test]
    fn a_parent_that_changed_is_not_taken_for_the_one_looked_at() {
        let other = std::os::unix::process::parent_id().wrapping_add(1);
        assert!(matches!(signal_probe(other), Outcome::Allowed(detail) if detail.contains("no longer")));
    }
}
