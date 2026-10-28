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
    worker: Option<std::sync::mpsc::Sender<String>>,
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
            if self.worker.is_none() {
                self.worker = spawn_worker();
            }
            if let Some(worker) = &self.worker
                && worker.send(text).is_err()
            {
                // The worker found no clipboard and stopped: don't start another.
                log::debug!("no primary selection to write");
            }
        }
        #[cfg(not(any(test, all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32"))))))]
        drop(text);
    }
}

/// A thread that owns the clipboard and writes what it is sent, the latest first: connecting to
/// the display and setting the selection make round trips to the compositor or X server, which
/// must never hold up a frame. Holding the clipboard also keeps the selection served on X11. It
/// stops when the app drops its sender, or when there is no clipboard to write.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
fn spawn_worker() -> Option<std::sync::mpsc::Sender<String>> {
    use arboard::{LinuxClipboardKind, SetExtLinux};
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let spawned = std::thread::Builder::new().name("primary-selection".into()).spawn(move || {
        let mut clipboard = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                log::debug!("no primary selection: {e}");
                return;
            }
        };
        while let Ok(mut text) = rx.recv() {
            // Selections sent while the last one was written are out of date but the newest.
            while let Ok(newer) = rx.try_recv() {
                text = newer;
            }
            if let Err(e) = clipboard.set().clipboard(LinuxClipboardKind::Primary).text(text) {
                log::debug!("couldn't set the primary selection: {e}");
            }
        }
    });
    match spawned {
        Ok(_) => Some(tx),
        Err(e) => {
            log::debug!("no primary selection thread: {e}");
            None
        }
    }
}
