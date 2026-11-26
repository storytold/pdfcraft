//! A confined launcher for helper processes that run code GridCraft doesn't trust, such as a
//! workbook's Python cells.
//!
//! The parent builds a [`Policy`] (the paths the program may read, and the programs it may
//! execute), creates a [`Canary`], and spawns the command [`launch_command`] returns. That command
//! runs the application itself again with [`LAUNCH_FLAG`]; the application hands its arguments to
//! [`handle_args`] before anything else, and the launcher:
//!
//! 1. closes every descriptor besides standard input, output and error, and refuses if one stays
//!    open: the confinement doesn't revoke what is already open, and an app's system libraries leave
//!    files open that a child inherits;
//! 2. confines itself: on Linux with Landlock (read-only access to the policy's paths, execute
//!    access only to its programs, no writes) and seccomp (no new processes, no sockets, none of
//!    the calls that escape or inspect a sandbox or that change a file where Landlock has no right
//!    for it); on macOS it runs itself again under
//!    `sandbox-exec` with a deny-by-default profile built from the same policy ([`CONFINED_FLAG`]);
//! 3. checks that the confinement holds: reading the canary, changing its permissions, truncating
//!    it, writing next to it, a TCP connection and starting a process must each be denied;
//! 4. only then replaces itself with the program.
//!
//! When any step fails, the program never runs: the launcher writes one line,
//! `gridcraft-sandbox: refused: <reason>`, to its standard error and exits with
//! [`REFUSED_EXIT_CODE`]. Other platforms (Windows, FreeBSD, the web) have no confinement here:
//! [`support`] says so and [`launch_command`] refuses, so callers show `#BLOCKED!`.
//!
//! The crate has no `unsafe` code: the system calls sit behind the `landlock`, `seccompiler` and
//! `nix` crates, and behind `/usr/bin/sandbox-exec` on macOS.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod canary;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod policy;
mod probe;

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

pub use canary::Canary;
pub use policy::Policy;
pub use probe::{Check, Outcome};

/// The first argument of a launcher run: the application passes its arguments to [`handle_args`].
pub const LAUNCH_FLAG: &str = "--sandbox-launch";
/// The first argument of a launcher already running confined (macOS, under `sandbox-exec`): it
/// goes straight to the enforcement check, which refuses when it isn't confined after all.
pub const CONFINED_FLAG: &str = "--sandbox-confined";
/// The first argument of the child the enforcement check tries to start; it exits at once.
pub const PROBE_CHILD_FLAG: &str = "--sandbox-probe-child";
/// The first argument of a confined run that reports what it may and may not do
/// (`gridcraft-cli sandbox-check`).
pub const REPORT_FLAG: &str = "--sandbox-report";
/// The exit code of a launcher that refused to run its program.
pub const REFUSED_EXIT_CODE: i32 = 97;

/// Why a program can't be run confined.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("this system has no supported sandbox: {0}")]
    Unsupported(String),
    #[error("bad sandbox policy: {0}")]
    BadPolicy(String),
    #[error("the sandbox is not enforced: {0}")]
    NotEnforced(String),
    #[error("couldn't start the sandbox: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, SandboxError>;

/// Whether this system can confine a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Support {
    /// The sandbox is available; the text names what provides it.
    Available(String),
    /// It is not, and why.
    Unsupported(String),
}

/// Whether this system can confine a program. A [`launch_command`] can still be refused when the
/// running kernel turns out not to enforce a restriction: the launcher checks again.
pub fn support() -> Support {
    platform::support()
}

/// The command that runs `program` with `args`, confined by `policy`, checked against `canary`.
/// The parent spawns it like any other command and keeps `canary` alive until the program has
/// started (its standard streams are the program's). Those stay open with the rights they were
/// opened with, which the confinement doesn't revoke: a stream that is a file can be written and
/// truncated, and one that is a socket can still send. So give the program pipes. The command runs the current executable
/// again, which must pass its arguments to [`handle_args`] first.
pub fn launch_command(policy: &Policy, canary: &Canary, program: &Path, args: &[OsString]) -> Result<Command> {
    launch_command_with(&std::env::current_exe()?, policy, canary, program, args)
}

/// [`launch_command`] with another executable as the launcher: one that also passes its arguments
/// to [`handle_args`] first.
pub fn launch_command_with(exe: &Path, policy: &Policy, canary: &Canary, program: &Path, args: &[OsString]) -> Result<Command> {
    let checked = policy.checked(exe, program, canary)?;
    let mut launch = vec![OsString::from(LAUNCH_FLAG)];
    launch.extend(checked.to_args(canary.file()));
    launch.push(OsString::from("--"));
    launch.push(checked.program().as_os_str().to_owned());
    launch.extend(args.iter().cloned());
    platform::command(&checked, exe, &launch)
}

/// Handles the application's own sandbox runs; call it with the process's arguments (without the
/// program name) before anything else. `None`: the arguments are not one of those runs, carry on.
/// `Some(code)`: exit with `code`. A launcher run whose checks pass doesn't return at all: it
/// becomes the program.
pub fn handle_args(args: &[OsString]) -> Option<i32> {
    let (first, rest) = args.split_first()?;
    if first == OsStr::new(LAUNCH_FLAG) || first == OsStr::new(CONFINED_FLAG) {
        let Err(e) = if first == OsStr::new(LAUNCH_FLAG) { launch(rest) } else { confined_again(rest) };
        eprintln!("gridcraft-sandbox: refused: {e}");
        return Some(REFUSED_EXIT_CODE);
    }
    if first == OsStr::new(PROBE_CHILD_FLAG) {
        return Some(0);
    }
    if first == OsStr::new(REPORT_FLAG) {
        return Some(probe::report(rest));
    }
    None
}

/// The launcher: check what it inherited, confine, check the confinement, become the program.
/// Returns only on failure.
fn launch(args: &[OsString]) -> Result<std::convert::Infallible> {
    // Arguments that can't be a launch are refused with nothing done. Reading them leaves nothing open.
    let (checked, _) = policy::Checked::from_args(args)?;
    // Before the launcher opens anything itself. On macOS this is also before the sandbox: under a
    // deny-by-default profile the system's libraries hold a socket of their own, which closes at
    // `exec` and which must not be taken for an inherited one.
    close_inherited_descriptors()?;
    // What the check tries on a process has to be allowed now, or its refusal then proves nothing.
    let target = probe::signal_target().map_err(SandboxError::NotEnforced)?;
    platform::confine(&checked, target, args)?;
    confined(target, args)
}

/// The launcher run again under the confinement ([`CONFINED_FLAG`]): the process the first run looked
/// at comes before its arguments.
fn confined_again(args: &[OsString]) -> Result<std::convert::Infallible> {
    let parsed = args.split_first().and_then(|(target, rest)| Some((target.to_str()?.parse().ok()?, rest)));
    let (target, rest) = parsed.ok_or_else(|| SandboxError::NotEnforced(format!("{CONFINED_FLAG} needs the process the launcher looked at")))?;
    confined(target, rest)
}

/// The launcher once it is confined: check that it holds, then become the program.
fn confined(target: u32, args: &[OsString]) -> Result<std::convert::Infallible> {
    let (checked, canary) = policy::Checked::from_args(args)?;
    let exe = std::env::current_exe()?;
    for check in probe::enforcement(&canary, &exe, target) {
        if let Outcome::Allowed(detail) = check.outcome {
            return Err(SandboxError::NotEnforced(format!("{}: {detail}", check.name)));
        }
    }
    platform::exec(checked.program(), checked.args())
}

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use unsupported as platform;

/// Platforms without confinement: every launch is refused.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported {
    use super::{Command, OsString, Path, Result, SandboxError, Support, policy::Checked};

    const WHY: &str = "GridCraft confines programs on Linux and macOS only";

    pub fn support() -> Support {
        Support::Unsupported(WHY.into())
    }

    pub fn command(_: &Checked, _: &Path, _: &[OsString]) -> Result<Command> {
        Err(SandboxError::Unsupported(WHY.into()))
    }

    pub fn confine(_: &Checked, _: u32, _: &[OsString]) -> Result<()> {
        Err(SandboxError::Unsupported(WHY.into()))
    }

    pub fn exec(_: &Path, _: &[OsString]) -> Result<std::convert::Infallible> {
        Err(SandboxError::Unsupported(WHY.into()))
    }
}

/// Closes every descriptor besides standard input, output and error, and refuses the launch if one
/// stays open or the list can't be read. One left open (a file outside the policy, a connected
/// socket) would survive `exec`, and the confinement doesn't revoke what is already open. Refusing
/// alone wouldn't do: in a desktop app the system's own libraries (Metal's shader cache on macOS)
/// leave files open without close-on-exec, and they reach every child.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn close_inherited_descriptors() -> Result<()> {
    for fd in descriptors_above_stdio()? {
        // The listing's own descriptor is on the list and is closed already; that error is fine.
        let _ = nix::unistd::close(fd);
    }
    let left = descriptors_above_stdio()?;
    // The second listing holds its own directory open, so one above stdio is expected.
    if left.len() > 1 {
        return Err(SandboxError::NotEnforced(format!("descriptors other than standard input, output and error stayed open ({left:?})")));
    }
    Ok(())
}

/// The open descriptors above standard error, the listing's own included.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn descriptors_above_stdio() -> Result<Vec<i32>> {
    let dir = if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" };
    let listing = std::fs::read_dir(dir).map_err(|e| SandboxError::NotEnforced(format!("can't list open descriptors ({dir}): {e}")))?;
    let mut above_stdio = Vec::new();
    for entry in listing {
        let entry = entry.map_err(|e| SandboxError::NotEnforced(format!("can't list open descriptors ({dir}): {e}")))?;
        match entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) {
            Some(fd) if fd > 2 => above_stdio.push(fd),
            Some(_) => {}
            None => return Err(SandboxError::NotEnforced(format!("unexpected entry in {dir}"))),
        }
    }
    above_stdio.sort_unstable();
    Ok(above_stdio)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn close_inherited_descriptors() -> Result<()> {
    Ok(())
}

/// Replaces this process with `program` (the confining platforms).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn exec_unix(program: &Path, args: &[OsString]) -> Result<std::convert::Infallible> {
    use std::os::unix::process::CommandExt as _;
    let mut cmd = Command::new(program);
    cmd.args(args).env_clear().env("PATH", CONFINED_PATH);
    for name in KEPT_ENV {
        if let Some(v) = std::env::var_os(name) {
            cmd.env(name, v);
        }
    }
    // `exec` returns only when it failed.
    Err(SandboxError::Io(cmd.exec()))
}

/// The only variables a confined program inherits, besides [`CONFINED_PATH`]: the locale and the
/// time zone. Everything else (tokens, credentials, the user's paths) stays with the parent.
pub const KEPT_ENV: [&str; 4] = ["LANG", "LC_ALL", "LC_CTYPE", "TZ"];

/// The `PATH` a confined program sees.
pub const CONFINED_PATH: &str = "/usr/bin:/bin";

#[cfg(test)]
mod tests;
