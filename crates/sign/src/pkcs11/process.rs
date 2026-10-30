//! Nonblocking bounded pipe I/O keeps driver hangs, crashes, and cancellation recoverable.
use super::worker::{ARGUMENT, LIMIT, Request};
use crate::SignError;
use std::cell::RefCell;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Cancellation shared with an owning UI/job. Cancelling never retries a card operation.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
thread_local! { static CURRENT: RefCell<Option<Cancellation>> = const { RefCell::new(None) }; }
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    /// Apply cancellation to synchronous card calls made by this job on this thread.
    pub fn run<T>(&self, job: impl FnOnce() -> T) -> T {
        struct Restore(Option<Cancellation>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|c| {
                    c.replace(self.0.take());
                });
            }
        }
        let _restore = Restore(CURRENT.with(|c| c.replace(Some(self.clone()))));
        job()
    }
    pub fn cancel_on_drop(&self) -> CancelOnDrop {
        CancelOnDrop(self.clone())
    }
    fn cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
/// Owned by a dialog/job so dropping it also stops a running helper.
pub struct CancelOnDrop(Cancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(super) fn io_error(error: std::io::Error) -> SignError {
    SignError::Crypto(format!("smartcard worker I/O failed: {error}"))
}

pub(super) fn request(request: &Request, pin: Option<&str>) -> Result<Zeroizing<Vec<u8>>, SignError> {
    let executable =
        std::env::var_os("PDFCRAFT_PKCS11_HELPER").map(std::path::PathBuf::from).map(Ok).unwrap_or_else(std::env::current_exe).map_err(io_error)?;
    let mut command = Command::new(executable);
    command.arg(ARGUMENT).env("PDFCRAFT_PKCS11_PARENT", std::process::id().to_string());
    let cancellation = CURRENT.with(|c| c.borrow().clone()).unwrap_or_default();
    let timeout = if matches!(request, Request::List { .. }) { Duration::from_secs(15) } else { Duration::from_secs(120) };
    let json = serde_json::to_vec(request).map_err(|_| SignError::Crypto("could not encode smartcard request".into()))?;
    if json.len() > LIMIT || pin.is_some_and(|p| p.len() > 256) {
        return Err(SignError::Malformed("smartcard request or PIN exceeds its limit".into()));
    }
    let mut input = Zeroizing::new(Vec::with_capacity(json.len().saturating_add(264)));
    input.extend_from_slice(&(json.len() as u32).to_be_bytes());
    input.extend_from_slice(&json);
    let pin = pin.unwrap_or("").as_bytes();
    input.extend_from_slice(&(pin.len() as u32).to_be_bytes());
    input.extend_from_slice(pin);
    exchange(command, &input, timeout, &cancellation)
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Reap even on timeout, cancellation, malformed output, or an I/O error.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn nonblocking(fd: &impl std::os::fd::AsFd) -> Result<(), SignError> {
    let flags = rustix::fs::fcntl_getfl(fd).map_err(|e| io_error(e.into()))?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK).map_err(|e| io_error(e.into()))
}
fn exchange(mut command: Command, input: &[u8], timeout: Duration, cancellation: &Cancellation) -> Result<Zeroizing<Vec<u8>>, SignError> {
    if cancellation.cancelled() {
        return Err(SignError::Crypto("smartcard operation cancelled".into()));
    }
    // Never give a provider the host PIN environment or an inherited terminal/log sink.
    command.env_remove("PDFCRAFT_PKCS11_PIN").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = ChildGuard(command.spawn().map_err(|e| {
        SignError::Crypto(format!("could not start smartcard worker: {e}; use a PdfCraft executable or configure PDFCRAFT_PKCS11_HELPER"))
    })?);
    let stdin = child.0.stdin.take().ok_or_else(|| SignError::Crypto("smartcard worker stdin is unavailable".into()))?;
    let mut stdout = child.0.stdout.take().ok_or_else(|| SignError::Crypto("smartcard worker stdout is unavailable".into()))?;
    nonblocking(&stdin)?;
    nonblocking(&stdout)?;
    let mut stdin = Some(stdin);
    let started = Instant::now();
    let mut written = 0;
    // Reserve once: reallocating a secret-bearing Vec could leave old copies unwiped.
    let mut response = Zeroizing::new(Vec::with_capacity(LIMIT.saturating_add(5)));
    let mut eof = false;
    loop {
        if cancellation.cancelled() {
            return Err(SignError::Crypto("smartcard operation cancelled; refresh the card before retrying".into()));
        }
        if started.elapsed() >= timeout {
            return Err(SignError::Crypto("smartcard operation timed out; check the card and PIN pad before retrying".into()));
        }
        if let Some(bytes) = input.get(written..).filter(|b| !b.is_empty())
            && let Some(pipe) = stdin.as_mut()
        {
            match pipe.write(bytes) {
                Ok(0) => return Err(SignError::Crypto("smartcard worker closed its input".into())),
                Ok(n) => written = written.saturating_add(n),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(io_error(e)),
            }
        }
        if written == input.len() {
            stdin = None;
        }
        // One read per iteration: output flooding must still observe cancellation/deadlines.
        let mut buffer = Zeroizing::new([0u8; 8192]);
        match stdout.read(buffer.as_mut()) {
            Ok(0) => eof = true,
            Ok(n) => {
                if response.len().saturating_add(n) > LIMIT.saturating_add(5) {
                    return Err(SignError::Crypto("smartcard worker response exceeds its limit".into()));
                }
                if let Some(bytes) = buffer.get(..n) {
                    response.extend_from_slice(bytes);
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {}
            Err(e) => return Err(io_error(e)),
        }
        if let Some(status) = child.0.try_wait().map_err(io_error)? {
            if !status.success() {
                return Err(SignError::Crypto("the smartcard worker stopped unexpectedly; check the provider and refresh cards".into()));
            }
            if eof {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = response.first().copied();
    let length = response.get(1..5).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_be_bytes).map(|n| n as usize);
    if length != Some(response.len().saturating_sub(5)) || !matches!(status, Some(0..=2)) {
        return Err(SignError::Crypto("invalid smartcard worker response".into()));
    }
    if status == Some(2) {
        return Err(SignError::TokenChanged);
    }
    let body = Zeroizing::new(response.get(5..).unwrap_or_default().to_vec());
    if status == Some(1) {
        return Err(SignError::Crypto(String::from_utf8_lossy(&body).chars().take(512).collect()));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }
    fn invoke(script: &str) -> Result<Zeroizing<Vec<u8>>, SignError> {
        exchange(shell(script), b"", Duration::from_secs(2), &Cancellation::default())
    }
    #[test]
    fn worker_crash_and_invalid_frames_are_errors() {
        assert!(invoke("kill -KILL $$").unwrap_err().to_string().contains("stopped unexpectedly"));
        assert!(invoke("printf bad").unwrap_err().to_string().contains("invalid"));
        assert!(invoke("printf '\\000\\000\\000\\000\\005x'").unwrap_err().to_string().contains("invalid"));
        assert!(invoke("printf '\\000\\000\\000\\000\\000extra'").unwrap_err().to_string().contains("invalid"));
        assert_eq!(&**invoke("printf '\\000\\000\\000\\000\\002ok'").unwrap(), b"ok");
    }
    #[test]
    fn stale_selection_status_is_preserved_as_a_typed_error() {
        assert!(matches!(invoke("printf '\\002\\000\\000\\000\\000'"), Err(SignError::TokenChanged)));
    }
    #[test]
    fn worker_deadline_is_enforced_even_when_the_driver_eventually_returns() {
        let result = exchange(shell("sleep 0.2; printf '\\000\\000\\000\\000\\000'"), b"", Duration::from_millis(30), &Cancellation::default());
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }
    #[test]
    fn closing_a_job_cancels_its_helper() {
        let cancellation = Cancellation::default();
        let owner = cancellation.cancel_on_drop();
        let thread = std::thread::spawn(move || exchange(shell("exec sleep 10"), b"", Duration::from_secs(2), &cancellation));
        std::thread::sleep(Duration::from_millis(40));
        let started = Instant::now();
        drop(owner);
        assert!(thread.join().unwrap().unwrap_err().to_string().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn oversized_worker_output_is_bounded() {
        assert!(invoke("exec head -c 1048582 /dev/zero").unwrap_err().to_string().contains("exceeds"));
    }
    #[test]
    fn inherited_pin_is_removed_from_worker_environment() {
        let mut command = shell("test -z \"${PDFCRAFT_PKCS11_PIN+x}\" || exit 7; printf '\\000\\000\\000\\000\\000'");
        command.env("PDFCRAFT_PKCS11_PIN", "disposable secret");
        assert!(exchange(command, b"", Duration::from_secs(2), &Cancellation::default()).is_ok());
    }
    #[test]
    fn pipe_payload_preserves_utf8_and_spaces_without_command_arguments() {
        let secret = " １２34 ".as_bytes();
        let mut frame = vec![0];
        frame.extend_from_slice(&(secret.len() as u32).to_be_bytes());
        frame.extend_from_slice(secret);
        let body = exchange(shell("exec cat"), &frame, Duration::from_secs(2), &Cancellation::default()).unwrap();
        assert_eq!(&**body, secret);
    }
    #[test]
    fn cancellation_scope_restores_after_nested_calls_and_unwinding() {
        let outer = Cancellation::default();
        let inner = Cancellation::default();
        outer.run(|| {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inner.run(|| panic!("synthetic job panic"))));
            outer.cancel();
            assert!(CURRENT.with(|c| c.borrow().as_ref().unwrap().cancelled()));
        });
        assert!(CURRENT.with(|c| c.borrow().is_none()));
    }
}
