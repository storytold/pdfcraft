//! The primary selection on Linux and the BSDs (#4821): the text selected on a page goes where a
//! middle click pastes it, as X11 and Wayland applications do (freedesktop.org clipboards spec;
//! Wayland's `primary-selection` protocol). Elsewhere there is no primary selection and this
//! does nothing.
//!
//! egui only writes the regular clipboard, so the text goes through arboard, which serves the
//! selection over Wayland's data-control protocol where the compositor offers it and over X11
//! (Xwayland included) otherwise.

use pdfcraft_engine::DocId;

use crate::canvas::SelectionKey;

/// The primary selection's writer, kept for the app's lifetime.
#[derive(Default)]
pub(crate) struct PrimarySelection {
    /// The page selection seen on the last frame: it is written once it holds still.
    pub(crate) seen: Option<(DocId, SelectionKey)>,
    /// The page selection written last, until the selection clears: the same text selected again
    /// takes the primary selection back from whichever application took it meanwhile.
    pub(crate) written_key: Option<(DocId, SelectionKey)>,
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
    worker: Option<Worker>,
    /// When the last worker was started: a stopped one is replaced no sooner than [`RETRY`]
    /// after, so a desktop with no clipboard isn't asked again on every selection.
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
    started: Option<std::time::Instant>,
    /// A selection made while a stopped worker waits to be replaced: written by the next one
    /// ([`PrimarySelection::retry`]).
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
    pending: Option<String>,
    /// Tests: what was written, oldest first.
    #[cfg(test)]
    pub(crate) written: Vec<String>,
}

impl PrimarySelection {
    /// Whether this platform has a primary selection.
    pub(crate) const SUPPORTED: bool = cfg!(any(test, all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")))));

    /// Make `text` the primary selection. A failure is logged, never shown: selecting text must
    /// keep working on a desktop without one.
    pub(crate) fn set(&mut self, text: String) {
        #[cfg(test)]
        self.written.push(text);
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
        {
            let mut text = text;
            if let Some(worker) = &self.worker {
                match worker.send(text) {
                    Ok(()) => return,
                    // The thread stopped (no clipboard then, or it failed): another takes over.
                    Err(back) => {
                        text = back;
                        self.worker = None;
                    }
                }
            }
            self.pending = Some(text);
            self.retry();
        }
        #[cfg(not(any(test, all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32"))))))]
        drop(text);
    }

    /// Write a selection made while a stopped worker waited to be replaced, once one may start.
    /// Run every frame; says how long until it may, so a frame can be drawn then.
    pub(crate) fn retry(&mut self) -> Option<std::time::Duration> {
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
        if self.pending.is_some() && self.worker.is_none() {
            let since = self.started.map(|at| at.elapsed());
            if !may_start(since) {
                return since.map(|since| RETRY.saturating_sub(since));
            }
            self.started = Some(std::time::Instant::now());
            self.worker = spawn_worker();
            if let (Some(worker), Some(text)) = (&self.worker, self.pending.take())
                && worker.send(text).is_err()
            {
                self.worker = None;
            }
        }
        None
    }
}

/// The thread that owns the clipboard, and the one selection waiting for it: setting it again
/// before the thread takes it replaces it, so a stalled display holds one string, not a queue.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
struct Worker {
    latest: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    wake: std::sync::mpsc::SyncSender<()>,
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
impl Worker {
    /// Hand the thread `text`. A full channel means the thread is already due to look; a closed
    /// one, that it found no clipboard or failed, and stopped: then `text` comes back.
    fn send(&self, text: String) -> Result<(), String> {
        *self.latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(text);
        match self.wake.try_send(()) {
            Err(std::sync::mpsc::TrySendError::Disconnected(())) => {
                Err(self.latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take().unwrap_or_default())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(any(test, all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")))))]
/// How long after a worker was started another may replace it.
const RETRY: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(any(test, all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")))))]
/// Whether a worker may be started, given how long ago the last one was (`None`: none was).
fn may_start(since_stopped: Option<std::time::Duration>) -> bool {
    since_stopped.is_none_or(|since| since >= RETRY)
}

/// A thread that owns the clipboard and writes the newest selection it is given: connecting to
/// the display and setting the selection make round trips to the compositor or X server, which
/// must never hold up a frame. Holding the clipboard also keeps the selection served on X11. It
/// stops when the app drops its [`Worker`], or when there is no clipboard to write.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
fn spawn_worker() -> Option<Worker> {
    use arboard::{LinuxClipboardKind, SetExtLinux};
    let latest = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let (wake, woken) = std::sync::mpsc::sync_channel::<()>(1);
    let slot = latest.clone();
    let spawned = std::thread::Builder::new().name("primary-selection".into()).spawn(move || {
        let mut clipboard = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                log::debug!("no primary selection: {e}");
                return;
            }
        };
        while woken.recv().is_ok() {
            let Some(text) = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() else { continue };
            if let Err(e) = clipboard.set().clipboard(LinuxClipboardKind::Primary).text(text) {
                log::debug!("couldn't set the primary selection: {e}");
            }
        }
    });
    match spawned {
        Ok(_) => Some(Worker { latest, wake }),
        Err(e) => {
            log::debug!("no primary selection thread: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RETRY, may_start};

    /// A worker found stopped (no clipboard, say) is started again, but not before a while has
    /// passed: a desktop with none isn't asked on every selection (#4821).
    #[test]
    fn a_stopped_worker_is_started_again_after_a_while() {
        assert!(may_start(None), "the first one");
        assert!(!may_start(Some(std::time::Duration::ZERO)));
        assert!(!may_start(Some(RETRY - std::time::Duration::from_millis(1))));
        assert!(may_start(Some(RETRY)));
    }
}
