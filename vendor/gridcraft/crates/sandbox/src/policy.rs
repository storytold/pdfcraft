//! What a confined program may touch, and how the launcher receives it on its command line.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::{Canary, Result, SandboxError};

/// At most this many paths in each list.
pub const MAX_PATHS: usize = 64;
/// At most this many bytes in one path.
pub const MAX_PATH_BYTES: usize = 4096;

/// The paths a confined program may read and the programs it may execute. Everything else is
/// denied: writing anywhere, network access and starting other processes have no switch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    read: Vec<PathBuf>,
    exec: Vec<PathBuf>,
}

impl Policy {
    pub fn new() -> Policy {
        Policy::default()
    }

    /// Lets the program read `path`: a file, or a directory and everything under it.
    pub fn read(mut self, path: impl Into<PathBuf>) -> Policy {
        self.read.push(path.into());
        self
    }

    /// Lets the program execute (and read) the file `path`.
    pub fn exec(mut self, path: impl Into<PathBuf>) -> Policy {
        self.exec.push(path.into());
        self
    }

    /// Adds what a dynamically linked program needs to start on this system: the shared-library
    /// directories and the dynamic loader's cache on Linux (macOS's base profile covers its own).
    /// Paths that don't exist here are skipped.
    pub fn with_system_libraries(mut self) -> Policy {
        if cfg!(target_os = "linux") {
            for dir in ["/lib", "/lib64", "/usr/lib", "/usr/lib64"] {
                // The dynamic loader is executed, so its directories get execute access.
                if Path::new(dir).exists() {
                    self.exec.push(dir.into());
                }
            }
            // What the C library and Python's `zoneinfo` read for the time zone.
            for path in ["/etc/ld.so.cache", "/etc/localtime", "/usr/share/zoneinfo"] {
                if Path::new(path).exists() {
                    self.read.push(path.into());
                }
            }
        }
        // Fonts, which a plot's text is drawn with. Only the system's: fonts in the home folder stay
        // out of reach. macOS's own `/System/Library/Fonts` comes with its base profile.
        let fonts: &[&str] = if cfg!(target_os = "linux") {
            &["/usr/share/fonts", "/usr/local/share/fonts"]
        } else if cfg!(target_os = "macos") {
            &["/Library/Fonts"]
        } else {
            &[]
        };
        for dir in fonts {
            if Path::new(dir).exists() {
                self.read.push((*dir).into());
            }
        }
        self
    }

    pub fn read_paths(&self) -> &[PathBuf] {
        &self.read
    }

    pub fn exec_paths(&self) -> &[PathBuf] {
        &self.exec
    }

    /// The policy as the launcher will apply it: every path absolute, existing and canonical, the
    /// lists within their caps, the current executable `exe` executable (the enforcement check
    /// starts it), `program` executable, and the canary outside every allowed path.
    pub(crate) fn checked(&self, exe: &Path, program: &Path, canary: &Canary) -> Result<Checked> {
        let read = canonical_all(&self.read)?;
        let mut exec = canonical_all(&self.exec)?;
        let program = canonical(program)?;
        for extra in [canonical(exe)?, program.clone()] {
            if !exec.contains(&extra) {
                exec.push(extra);
            }
        }
        if exec.len() > MAX_PATHS {
            return Err(SandboxError::BadPolicy(format!("more than {MAX_PATHS} executable paths")));
        }
        let probe = canonical(canary.file())?;
        if read.iter().chain(&exec).any(|p| probe.starts_with(p)) {
            return Err(SandboxError::BadPolicy(format!("the canary {} is inside an allowed path", probe.display())));
        }
        Ok(Checked { read, exec, program, args: Vec::new() })
    }
}

fn canonical_all(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    if paths.len() > MAX_PATHS {
        return Err(SandboxError::BadPolicy(format!("more than {MAX_PATHS} paths")));
    }
    let mut out: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for p in paths {
        let c = canonical(p)?;
        if !out.contains(&c) {
            out.push(c);
        }
    }
    Ok(out)
}

/// `path` made canonical, after refusing what can't be one: an empty or relative path, one over
/// [`MAX_PATH_BYTES`], one with a NUL or a line break (which no sandbox profile can quote), and
/// one that doesn't exist.
fn canonical(path: &Path) -> Result<PathBuf> {
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.is_empty() || !path.is_absolute() {
        return Err(SandboxError::BadPolicy(format!("{} is not an absolute path", path.display())));
    }
    if bytes.len() > MAX_PATH_BYTES {
        return Err(SandboxError::BadPolicy(format!("a path is longer than {MAX_PATH_BYTES} bytes")));
    }
    if bytes.iter().any(|b| matches!(b, 0 | b'\n' | b'\r')) {
        return Err(SandboxError::BadPolicy("a path holds a NUL or a line break".into()));
    }
    let c = path.canonicalize().map_err(|e| SandboxError::BadPolicy(format!("{}: {e}", path.display())))?;
    if c.as_os_str().as_encoded_bytes().len() > MAX_PATH_BYTES {
        return Err(SandboxError::BadPolicy(format!("a path is longer than {MAX_PATH_BYTES} bytes")));
    }
    Ok(c)
}

/// A policy checked by [`Policy::checked`], with the program and its arguments: what the launcher
/// applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Checked {
    pub read: Vec<PathBuf>,
    pub exec: Vec<PathBuf>,
    program: PathBuf,
    args: Vec<OsString>,
}

impl Checked {
    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    /// The launcher's options for this policy (`--read`, `--exec`, `--probe`), without the
    /// program, which follows a `--`.
    pub fn to_args(&self, canary: &Path) -> Vec<OsString> {
        let mut out = Vec::new();
        for (flag, paths) in [("--read", &self.read), ("--exec", &self.exec)] {
            for p in paths {
                out.push(OsString::from(flag));
                out.push(p.as_os_str().to_owned());
            }
        }
        out.push(OsString::from("--probe"));
        out.push(canary.as_os_str().to_owned());
        out
    }

    /// Reads the launcher's arguments back: options, `--`, the program and its arguments. The
    /// paths are checked again (the launcher trusts nothing), and the canary is required.
    pub fn from_args(args: &[OsString]) -> Result<(Checked, PathBuf)> {
        let mut read = Vec::new();
        let mut exec = Vec::new();
        let mut probe: Option<PathBuf> = None;
        let mut it = args.iter();
        loop {
            let Some(flag) = it.next() else { return Err(SandboxError::BadPolicy("no program after `--`".into())) };
            if flag == OsStr::new("--") {
                break;
            }
            let value = it.next().ok_or_else(|| SandboxError::BadPolicy(format!("{} needs a path", flag.to_string_lossy())))?;
            let path = canonical(Path::new(value))?;
            match flag.to_str() {
                Some("--read") => read.push(path),
                Some("--exec") => exec.push(path),
                Some("--probe") if probe.is_none() => probe = Some(path),
                _ => return Err(SandboxError::BadPolicy(format!("unknown launcher option {}", flag.to_string_lossy()))),
            }
            if read.len() > MAX_PATHS || exec.len() > MAX_PATHS {
                return Err(SandboxError::BadPolicy(format!("more than {MAX_PATHS} paths")));
            }
        }
        let program = it.next().ok_or_else(|| SandboxError::BadPolicy("no program after `--`".into()))?;
        let program = canonical(Path::new(program))?;
        if !exec.contains(&program) {
            return Err(SandboxError::BadPolicy(format!("{} is not an executable path of the policy", program.display())));
        }
        let probe = probe.ok_or_else(|| SandboxError::BadPolicy("no canary (`--probe`)".into()))?;
        if read.iter().chain(&exec).any(|p| probe.starts_with(p)) {
            return Err(SandboxError::BadPolicy(format!("the canary {} is inside an allowed path", probe.display())));
        }
        // The enforcement check starts the current executable. Outside the executable paths, the file
        // rules would refuse that start, and the check would prove nothing about new processes.
        let exe = canonical(&std::env::current_exe()?)?;
        if !exec.iter().any(|p| exe.starts_with(p)) {
            return Err(SandboxError::BadPolicy(format!("the current executable {} is not an executable path of the policy", exe.display())));
        }
        Ok((Checked { read, exec, program, args: it.cloned().collect() }, probe))
    }
}
