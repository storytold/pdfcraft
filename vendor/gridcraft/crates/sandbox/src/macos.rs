//! macOS: the launcher runs itself again under `sandbox-exec` with a deny-by-default profile built
//! from the policy, checks that the profile holds, and `exec`s the program, which stays under it.

use std::ffi::OsString;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::Command;

use crate::policy::Checked;
use crate::{CONFINED_FLAG, Result, SandboxError, Support};

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

pub fn support() -> Support {
    if Path::new(SANDBOX_EXEC).exists() {
        Support::Available("sandbox-exec with a deny-by-default profile (checked again at each launch)".into())
    } else {
        Support::Unsupported(format!("{SANDBOX_EXEC} is missing"))
    }
}

/// The launcher starts unconfined, so it can look at what it inherited first ([`confine`]). A
/// policy the profile can't express is refused here, before anything starts.
pub fn command(policy: &Checked, exe: &Path, launch: &[OsString]) -> Result<Command> {
    if !Path::new(SANDBOX_EXEC).exists() {
        return Err(SandboxError::Unsupported(format!("{SANDBOX_EXEC} is missing")));
    }
    profile(policy)?;
    let mut c = Command::new(exe);
    c.args(launch);
    Ok(c)
}

/// Runs the launcher again under `sandbox-exec`, with the profile applied before it starts; the
/// enforcement check of that run proves it. Returns only on failure.
pub fn confine(policy: &Checked, target: u32, args: &[OsString]) -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut c = Command::new(SANDBOX_EXEC);
    c.arg("-p").arg(profile(policy)?).arg(exe).arg(CONFINED_FLAG).arg(target.to_string()).args(args);
    Err(SandboxError::Io(c.exec()))
}

pub fn exec(program: &Path, args: &[OsString]) -> Result<std::convert::Infallible> {
    crate::exec_unix(program, args)
}

/// The profile: everything is denied except what the base system profile allows a process to
/// start (`system.sb`: the system libraries, the dynamic loader's cache and a few system
/// services), reading the policy's paths, executing its programs, reading file metadata (path
/// resolution), reading system settings (`sysctl`) and writing `/dev/null`. There is no rule for
/// starting processes, writing files or the network, so they stay denied.
pub fn profile(policy: &Checked) -> Result<String> {
    let mut p = String::from("(version 1)\n(deny default)\n(import \"system.sb\")\n");
    p.push_str("(allow process-exec");
    for path in &policy.exec {
        p.push_str(&path_rule(path)?);
    }
    p.push_str(")\n(allow file-read*");
    for path in policy.exec.iter().chain(&policy.read) {
        p.push_str(&path_rule(path)?);
    }
    p.push_str(")\n(allow file-read-metadata)\n(allow sysctl-read)\n(allow file-write-data (literal \"/dev/null\"))\n");
    Ok(p)
}

/// A profile rule for `path`. A literal matches only that path, so a directory, whose contents the
/// policy means, as Landlock takes it on Linux, is a subpath.
fn path_rule(path: &Path) -> Result<String> {
    let rule = if path.is_dir() { "subpath" } else { "literal" };
    Ok(format!(" ({rule} {})", quoted(path)?))
}

/// `path` as a profile string literal: in double quotes, with `"` and `\` escaped. Paths with
/// other control characters (the policy already refuses NUL and line breaks) or that aren't UTF-8
/// are refused rather than quoted.
pub fn quoted(path: &Path) -> Result<String> {
    let s = path.to_str().ok_or_else(|| SandboxError::BadPolicy(format!("{} is not UTF-8", path.display())))?;
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => return Err(SandboxError::BadPolicy(format!("{} holds a control character", path.display()))),
            c => out.push(c),
        }
    }
    out.push('"');
    Ok(out)
}
