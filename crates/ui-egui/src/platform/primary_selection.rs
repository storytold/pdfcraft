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

    /// Drop the selections waiting to be written or tried again: once the window loses the focus,
    /// the user may have selected text elsewhere, which a late write must not replace.
    pub(crate) fn cancel_retries(&mut self) {
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
        {
            self.pending = None;
            if let Some(worker) = &self.worker {
                worker.cancel();
            }
        }
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
    /// A selection the display refused, tried again every [`RETRY`] until it is written, a newer
    /// one replaces it, or the window loses the focus ([`PrimarySelection::cancel_retries`]).
    refused: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// Bumped when the window loses the focus: a write under way then that fails isn't kept for
    /// another try.
    cancelled: std::sync::Arc<std::sync::atomic::AtomicU64>,
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

    /// Forget what waits to be written, a selection the display refused included, and keep a
    /// write under way from being tried again.
    fn cancel(&self) {
        self.cancelled.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        self.refused.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    }
}

#[cfg(any(test, all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")))))]
/// How long before trying again: to replace a stopped worker (from when it was started), or to
/// write again a selection the display refused.
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
    let refused = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (wake, woken) = std::sync::mpsc::sync_channel::<()>(1);
    let (slot, failed, generation) = (latest.clone(), refused.clone(), cancelled.clone());
    let spawned = std::thread::Builder::new().name("primary-selection".into()).spawn(move || {
        let mut clipboard = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                log::debug!("no primary selection: {e}");
                return;
            }
        };
        // Woken for each selection, and every `RETRY` to write again one the display refused. A
        // new selection replaces a refused one.
        while !matches!(woken.recv_timeout(RETRY), Err(std::sync::mpsc::RecvTimeoutError::Disconnected)) {
            let started = generation.load(std::sync::atomic::Ordering::SeqCst);
            let new = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            let mut refused = failed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(text) = new.or_else(|| refused.take()) else { continue };
            *refused = None;
            drop(refused);
            if let Err(e) = clipboard.set().clipboard(LinuxClipboardKind::Primary).text(text.clone()) {
                log::debug!("couldn't set the primary selection: {e}");
                // Under the lock `cancel` takes after bumping the generation, so a cancel either
                // sees this text and drops it, or has bumped it already and this keeps nothing.
                let mut refused = failed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if generation.load(std::sync::atomic::Ordering::SeqCst) == started {
                    *refused = Some(text);
                }
            }
        }
    });
    match spawned {
        Ok(_) => Some(Worker { latest, refused, cancelled, wake }),
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
