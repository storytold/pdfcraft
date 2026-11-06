//! Platform-specific inheritance behind maintained safe wrappers; never a debug socket.

use std::io::{Read, Write};
use std::path::Path;

type Pipes = (Child, Box<dyn Write + Send>, Box<dyn Read + Send>);

#[cfg(unix)]
pub(super) struct Child(std::process::Child);

#[cfg(unix)]
impl Drop for Child {
    fn drop(&mut self) {
        if let Ok(pid) = i32::try_from(self.0.id())
            && let Some(pid) = rustix::process::Pid::from_raw(pid)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(target_os = "windows")]
pub(super) struct Child {
    _process: win_custody::Process,
    _job: win_custody::Job,
}

pub(super) fn launch(executable: &Path, profile: &Path, headless_test: bool) -> Result<Pipes, String> {
    let mut args = vec![
        "--remote-debugging-pipe".to_string(),
        format!("--user-data-dir={}", profile.to_str().ok_or("the temporary browser profile path is not Unicode")?),
        "--no-first-run".into(),
        "--incognito".into(),
        "--no-default-browser-check".into(),
        "--disable-background-networking".into(),
        "--disable-component-update".into(),
        "--disable-sync".into(),
        "--disable-extensions".into(),
    ];
    if headless_test {
        args.push("--headless=new".into());
    }
    args.push("about:blank".into());
    launch_os(executable, &args).map_err(|_| "could not launch the installed signing browser with private pipes".into())
}

#[cfg(unix)]
fn launch_os(executable: &Path, args: &[String]) -> std::io::Result<Pipes> {
    use command_fds::{CommandFdExt as _, FdMapping};
    use std::os::unix::process::CommandExt as _;
    let (input, writer) = os_pipe::pipe()?;
    let (reader, output) = os_pipe::pipe()?;
    let mut command = std::process::Command::new(executable);
    command.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).process_group(0);
    command
        .fd_mappings(vec![FdMapping { parent_fd: input.into(), child_fd: 3 }, FdMapping { parent_fd: output.into(), child_fd: 4 }])
        .map_err(|_| std::io::Error::other("invalid browser pipe mapping"))?;
    let child = command.spawn()?;
    // Command owns duplicate child pipe ends until dropped; close them before reading EOF.
    drop(command);
    Ok((Child(child), Box::new(writer), Box::new(reader)))
}

#[cfg(target_os = "windows")]
fn launch_os(executable: &Path, args: &[String]) -> std::io::Result<Pipes> {
    let (input, writer) = win_custody::anonymous_pipe()?;
    let (reader, output) = win_custody::anonymous_pipe()?;
    let job = win_custody::Job::create(&win_custody::JobLimits::new().kill_on_close(true))?;
    let mut command = win_custody::Launch::new(executable);
    let read_handle = command.inherit(&input)?;
    let write_handle = command.inherit(&output)?;
    command.args(args).arg(format!("--remote-debugging-io-pipes={read_handle},{write_handle}")).create_no_window(true);
    let process = command.spawn_in_job(&job)?;
    drop(command);
    Ok((Child { _process: process, _job: job }, Box::new(std::fs::File::from(writer)), Box::new(std::fs::File::from(reader))))
}
