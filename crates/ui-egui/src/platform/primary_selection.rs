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

/// The primary selection's writer, kept for the app's lifetime: on X11 the selection is served
/// only while a clipboard handle lives.
#[derive(Default)]
pub(crate) struct PrimarySelection {
    /// The page selection seen on the last frame: it is written once it holds still.
    pub(crate) seen: Option<(DocId, SelectionKey)>,
    /// The page selection written last.
    pub(crate) written_key: Option<(DocId, SelectionKey)>,
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
    clipboard: Option<arboard::Clipboard>,
    /// The system has no clipboard to write (no display, no data-control or X11): don't ask again.
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32")), not(test)))]
    unavailable: bool,
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
            use arboard::{LinuxClipboardKind, SetExtLinux};
            if self.unavailable {
                return;
            }
            if self.clipboard.is_none() {
                match arboard::Clipboard::new() {
                    Ok(c) => self.clipboard = Some(c),
                    Err(e) => {
                        log::debug!("no primary selection: {e}");
                        self.unavailable = true;
                        return;
                    }
                }
            }
            let Some(clipboard) = self.clipboard.as_mut() else { return };
            if let Err(e) = clipboard.set().clipboard(LinuxClipboardKind::Primary).text(text) {
                log::debug!("couldn't set the primary selection: {e}");
            }
        }
        #[cfg(not(any(test, all(unix, not(any(target_os = "macos", target_os = "android", target_arch = "wasm32"))))))]
        drop(text);
    }
}
