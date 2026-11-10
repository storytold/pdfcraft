//! One task pass per process. Only the child accesses document/network paths.
//! The supervisor never waits indefinitely for OCR, a network filesystem or a pipe.
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use pdfcraft_engine::hot_folder as hf;
use serde::{Deserialize, Serialize};

const START: &[u8] = b"PDFCRAFT-TASK-1\n";
const TICK: Duration = Duration::from_millis(100);
const STOP_GRACE: Duration = Duration::from_secs(3);
const REAP_LIMIT: Duration = Duration::from_secs(5);
const MAX_REPLY: u64 = 2048;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub task_timeout_seconds: u64,
    pub memory_limit_mib: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { task_timeout_seconds: 300, memory_limit_mib: 4096 }
    }
}

impl Limits {
    pub fn load(config: &Path) -> Result<Self, String> {
        let path = config.with_extension("worker.json");
        crate::worker::local_file(&path)?;
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("Cannot read worker limits: {error}")),
        };
        if file.metadata().map_err(|e| e.to_string())?.len() > 4096 {
            return Err("worker limits exceed 4096 bytes".into());
        }
        let limits: Self = serde_json::from_reader(file.take(4097)).map_err(|e| format!("Invalid worker limits: {e}"))?;
        if !(10..=86400).contains(&limits.task_timeout_seconds) || !(256..=65536).contains(&limits.memory_limit_mib) {
            return Err("worker limits require task_timeout_seconds 10..86400 and memory_limit_mib 256..65536".into());
        }
        Ok(limits)
    }

    fn policy(&self) -> Policy {
        Policy {
            timeout: Duration::from_secs(self.task_timeout_seconds),
            grace: STOP_GRACE,
            reap: REAP_LIMIT,
            memory_bytes: self.memory_limit_mib.saturating_mul(1024 * 1024),
        }
    }
}

#[derive(Clone, Copy)]
struct Policy {
    timeout: Duration,
    grace: Duration,
    reap: Duration,
    memory_bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    error: Option<String>,
}

/// A child cannot start work until the parent attached its Windows job and sent START.
/// No credential or privileged operation is exposed by this internal mode: it retains
/// the invoking account, requires a held supervisor lock, and validates the saved task.
pub fn child_main(config: &Path, task_id: &str) -> Result<(), String> {
    crate::worker::check_identity()?;
    let stop = read_start()?;
    crate::worker::local_file(config)?;
    let lock = std::fs::OpenOptions::new().read(true).write(true).open(config.with_extension("worker.lock")).map_err(|e| e.to_string())?;
    match lock.try_lock() {
        Err(std::fs::TryLockError::WouldBlock) => {}
        _ => return Err("task children require an active supervisor".into()),
    }
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hf::poll_task_cancelable(config, None, task_id, &|| stop.load(Ordering::Relaxed))))
            .unwrap_or_else(|_| Err("the isolated task failed safely; durable receipts and originals were retained".into()));
    // Keep the reply below the anonymous pipe capacity. Diagnostics are JSON, never commands.
    reply(result.err())
}

fn read_start() -> Result<Arc<AtomicBool>, String> {
    let mut prefix = [0; START.len()];
    std::io::stdin().read_exact(&mut prefix).map_err(|_| "task child requires its supervisor input pipe")?;
    if prefix != START {
        return Err("invalid task child protocol".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    std::thread::Builder::new()
        .name("task-cancellation".into())
        .spawn(move || {
            // A cancellation byte, closed pipe (including parent death), or read error stops work.
            let _ = std::io::stdin().read(&mut [0]);
            signal.store(true, Ordering::SeqCst);
        })
        .map_err(|e| format!("Cannot start cancellation monitor: {e}"))?;
    Ok(stop)
}

fn reply(error: Option<String>) -> Result<(), String> {
    // 300 scalar values need at most 1800 JSON bytes, even for escaped control characters.
    let reply = Reply { error: error.map(|message| message.chars().take(300).collect()) };
    let bytes = serde_json::to_vec(&reply).map_err(|e| e.to_string())?;
    std::io::stdout().write_all(&bytes).and_then(|_| std::io::stdout().flush()).map_err(|e| e.to_string())
}

pub fn run(config: &Path, task_id: &str, limits: &Limits, should_stop: &dyn Fn() -> bool) -> Result<Outcome, String> {
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command.arg("--task-child").arg("--config").arg(config).arg("--task-id").arg(task_id);
    supervise(&mut command, limits.policy(), should_stop)
}

struct Running {
    child: Child,
    reply: std::sync::mpsc::Receiver<Result<Vec<u8>, String>>,
    reaped: bool,
    termination_attempted: bool,
    #[cfg(windows)]
    job: Option<processkit::ProcessGroup>,
}

impl Running {
    fn start(command: &mut Command, memory_bytes: u64) -> Result<Self, String> {
        #[cfg(not(windows))]
        let _ = memory_bytes; // Unix uses sampled resident memory, not a kernel commit limit.
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW.
        }
        let mut child = command.spawn().map_err(|e| format!("Cannot start isolated task: {e}"))?;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let stdout = child.stdout.take();
        let mut running = Self {
            child,
            reply: receiver,
            reaped: false,
            termination_attempted: false,
            #[cfg(windows)]
            job: None,
        };
        let stdout = stdout.ok_or("task reply pipe is unavailable")?;
        std::thread::Builder::new()
            .name("task-reply".into())
            .spawn(move || {
                let mut bytes = Vec::new();
                let result = stdout.take(MAX_REPLY + 1).read_to_end(&mut bytes).map(|_| bytes).map_err(|e| e.to_string());
                let _ = sender.send(result);
            })
            .map_err(|e| format!("Cannot start task reply monitor: {e}"))?;
        #[cfg(windows)]
        {
            // JOB_OBJECT_LIMIT_JOB_MEMORY caps private committed memory before START,
            // including rapid allocations between monitor ticks and any descendants.
            // The safe wrapper also sets KILL_ON_JOB_CLOSE for supervisor crashes.
            let options = processkit::ProcessGroupOptions::default().max_memory(memory_bytes);
            let job = processkit::ProcessGroup::with_options(options).map_err(|e| format!("Cannot limit task process job: {e}"))?;
            job.adopt_external(running.child.id()).map_err(|e| format!("Cannot isolate task process: {e}"))?;
            running.job = Some(job);
        }
        running
            .child
            .stdin
            .as_mut()
            .ok_or("task control pipe is unavailable")?
            .write_all(START)
            .map_err(|e| format!("Cannot start task protocol: {e}"))?;
        Ok(running)
    }

    fn status(&mut self) -> Result<Option<ExitStatus>, String> {
        let status = self.child.try_wait().map_err(|e| format!("Cannot inspect isolated task: {e}"))?;
        self.reaped |= status.is_some();
        Ok(status)
    }

    fn cancel(&mut self) {
        // Closing stdin is non-blocking and cannot fill a child-owned pipe.
        self.child.stdin.take();
    }

    fn terminate(&mut self, reap_limit: Duration) -> Result<(), String> {
        self.termination_attempted = true;
        self.cancel();
        if self.status().is_ok_and(|status| status.is_some()) {
            return Ok(());
        }
        #[cfg(windows)]
        self.job.take(); // Kill every member and release the job even if the child crashed.
        let kill = self.child.kill();
        let started = Instant::now();
        loop {
            if self.status()?.is_some() {
                return Ok(());
            }
            if started.elapsed() >= reap_limit {
                return Err(format!(
                    "the operating system did not reap task PID {} within {} seconds after termination ({kill:?}); supervisor must restart",
                    self.child.id(),
                    reap_limit.as_secs()
                ));
            }
            std::thread::sleep(TICK);
        }
    }

    fn outcome(&mut self, status: ExitStatus) -> Outcome {
        if !status.success() {
            return Outcome::Failed(format!("isolated task exited unexpectedly ({status}); source cleanup resumes only from durable receipts"));
        }
        #[cfg(windows)]
        self.job.take(); // Also close inherited output handles in any unexpected descendants.
        let bytes = match self.reply.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(bytes)) if bytes.len() as u64 <= MAX_REPLY => bytes,
            _ => return Outcome::Failed("isolated task returned an invalid, missing or oversized reply".into()),
        };
        match serde_json::from_slice::<Reply>(&bytes) {
            Ok(Reply { error: None }) => Outcome::Completed,
            Ok(Reply { error: Some(error) }) => Outcome::Failed(error),
            Err(_) => Outcome::Failed("isolated task did not return a valid completion reply".into()),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.reaped && !self.termination_attempted {
            // Covers startup failures and unwinding without leaving an unsupervised worker.
            let _ = self.terminate(REAP_LIMIT);
        }
    }
}

struct Memory {
    #[cfg(not(windows))]
    system: sysinfo::System,
}

impl Memory {
    fn new() -> Self {
        Self {
            #[cfg(not(windows))]
            system: sysinfo::System::new(),
        }
    }

    fn bytes(&mut self, running: &Running) -> Result<u64, String> {
        #[cfg(windows)]
        {
            // Peak committed bytes include paged-out allocations and descendants.
            running
                .job
                .as_ref()
                .ok_or("task process job is unavailable")?
                .stats()
                .map_err(|e| e.to_string())?
                .peak_memory_bytes
                .ok_or_else(|| "job memory information is unavailable".into())
        }
        #[cfg(not(windows))]
        {
            let pid = sysinfo::Pid::from_u32(running.child.id());
            self.system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing().with_memory(),
            );
            self.system.process(pid).map(|process| process.memory()).ok_or_else(|| "process memory information is unavailable".into())
        }
    }
}

fn supervise(command: &mut Command, policy: Policy, should_stop: &dyn Fn() -> bool) -> Result<Outcome, String> {
    if should_stop() {
        return Ok(Outcome::Cancelled);
    }
    let mut running = Running::start(command, policy.memory_bytes)?;
    let mut memory = Memory::new();
    let started = Instant::now();
    let mut cancellation: Option<(Instant, Outcome)> = None;
    loop {
        if let Some(status) = running.status()? {
            return Ok(cancellation.map_or_else(|| running.outcome(status), |(_, outcome)| outcome));
        }
        if cancellation.is_none() {
            let reason = if should_stop() {
                Some(Outcome::Cancelled)
            } else if started.elapsed() >= policy.timeout {
                Some(Outcome::Failed(format!(
                    "task pass exceeded {} seconds; increase task_timeout_seconds in the local worker limits for larger jobs",
                    policy.timeout.as_secs()
                )))
            } else {
                match memory.bytes(&running) {
                    Ok(bytes) if bytes > policy.memory_bytes => {
                        // A runaway allocation must not continue during the ordinary stop grace.
                        running.terminate(policy.reap)?;
                        return Ok(Outcome::Failed(format!(
                            "task exceeded the monitored memory limit ({} MiB)",
                            policy.memory_bytes / (1024 * 1024)
                        )));
                    }
                    Ok(_) => None,
                    Err(error) => {
                        if let Some(status) = running.status()? {
                            return Ok(running.outcome(status));
                        }
                        running.terminate(policy.reap)?;
                        return Ok(Outcome::Failed(format!("cannot safely monitor task memory: {error}")));
                    }
                }
            };
            if let Some(reason) = reason {
                running.cancel();
                cancellation = Some((Instant::now(), reason));
            }
        }
        if cancellation.as_ref().is_some_and(|(when, _)| when.elapsed() >= policy.grace) {
            running.terminate(policy.reap)?;
            return Ok(cancellation.map_or(Outcome::Cancelled, |(_, outcome)| outcome));
        }
        std::thread::sleep(TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(scenario: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "worker_process::tests::synthetic_child_entry", "--nocapture"]);
        command.env("PDFCRAFT_WORKER_TEST_SCENARIO", scenario);
        command
    }

    fn policy() -> Policy {
        Policy {
            timeout: Duration::from_millis(700),
            grace: Duration::from_millis(150),
            reap: Duration::from_secs(3),
            memory_bytes: 4 * 1024 * 1024 * 1024,
        }
    }

    #[test]
    fn synthetic_child_entry() {
        let Ok(scenario) = std::env::var("PDFCRAFT_WORKER_TEST_SCENARIO") else { return };
        let stop = read_start().unwrap();
        match scenario.as_str() {
            "hang" => loop {
                std::thread::park();
            },
            "crash" => std::process::exit(29),
            "memory-refused" => {
                let mut bytes = Vec::<u8>::new();
                std::process::exit(if bytes.try_reserve_exact(128 * 1024 * 1024).is_err() { 91 } else { 92 });
            }
            "memory" => {
                let bytes = vec![42_u8; 128 * 1024 * 1024];
                std::hint::black_box(&bytes);
                loop {
                    std::thread::park();
                }
            }
            "stop" => {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            _ => {}
        }
        // The harness writes banners to stdout, so a dedicated exit avoids its completion output.
        // The supervisor tests use failure/stop paths and inspect the process lifetime, not this reply.
        std::process::exit(0);
    }

    #[test]
    fn hung_task_is_killed_reaped_and_does_not_prevent_the_next_task() {
        let started = Instant::now();
        let result = supervise(&mut synthetic("hang"), policy(), &|| false).unwrap();
        assert!(matches!(result, Outcome::Failed(message) if message.contains("exceeded")));
        assert!(started.elapsed() < Duration::from_secs(6));
        let next = supervise(&mut synthetic("crash"), policy(), &|| false).unwrap();
        assert!(matches!(next, Outcome::Failed(message) if message.contains("unexpectedly")));
    }

    #[test]
    fn stop_cancels_a_cooperative_child_and_kills_a_hung_one_within_the_bound() {
        for scenario in ["stop", "hang"] {
            let started = Instant::now();
            let result = supervise(&mut synthetic(scenario), policy(), &|| started.elapsed() >= Duration::from_millis(200)).unwrap();
            assert_eq!(result, Outcome::Cancelled);
            assert!(started.elapsed() < Duration::from_secs(5));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn excessive_child_memory_is_detected_and_reaped() {
        let mut policy = policy();
        policy.timeout = Duration::from_secs(10);
        policy.memory_bytes = 64 * 1024 * 1024;
        let result = supervise(&mut synthetic("memory"), policy, &|| false).unwrap();
        assert!(matches!(result, Outcome::Failed(ref message) if message.contains("memory limit")), "{result:?}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_job_refuses_excess_commit_before_the_monitor_can_sample() {
        let mut running = Running::start(&mut synthetic("memory-refused"), 64 * 1024 * 1024).unwrap();
        let started = Instant::now();
        let status = loop {
            if let Some(status) = running.status().unwrap() {
                break status;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "allocation helper failed to finish");
            std::thread::sleep(TICK);
        };
        // The helper uses fallible allocation and reports the actual refusal;
        // an allocator abort or generic crash would not satisfy this assertion.
        assert_eq!(status.code(), Some(91));
    }

    #[test]
    fn stopped_supervisor_never_launches_a_child() {
        assert_eq!(supervise(&mut Command::new("does-not-exist"), policy(), &|| true).unwrap(), Outcome::Cancelled);
    }

    #[test]
    fn limits_are_local_bounded_and_reject_unknown_or_out_of_range_values() {
        let base = tempfile::tempdir().unwrap();
        let config = base.path().join("tasks.json");
        let path = config.with_extension("worker.json");
        assert_eq!(Limits::load(&config).unwrap().task_timeout_seconds, 300);
        for invalid in [
            r#"{"task_timeout_seconds":0}"#,
            r#"{"task_timeout_seconds":86401}"#,
            r#"{"memory_limit_mib":255}"#,
            r#"{"memory_limit_mib":65537}"#,
            r#"{"misspelled_limit":123}"#,
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(Limits::load(&config).is_err(), "{invalid}");
        }
        std::fs::write(&path, vec![b' '; 4097]).unwrap();
        assert!(Limits::load(&config).unwrap_err().contains("4096"));
        std::fs::write(&path, r#"{"task_timeout_seconds":900,"memory_limit_mib":2048}"#).unwrap();
        let limits = Limits::load(&config).unwrap();
        assert_eq!(limits.task_timeout_seconds, 900);
        assert_eq!(limits.policy().memory_bytes, 2048 * 1024 * 1024);
    }
}
