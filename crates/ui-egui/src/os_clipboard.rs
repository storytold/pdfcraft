//! The one way to the operating system's clipboard.
//!
//! The system clipboard is a single process-wide resource: on Windows, two threads that
//! open it at once can corrupt the heap. Every access goes through [`with`], which holds a
//! lock across the whole open, use and close, so parallel callers (test threads, workers)
//! take turns instead of racing.

use std::sync::{Mutex, PoisonError};

static LOCK: Mutex<()> = Mutex::new(());

/// Opens the system clipboard, runs `f` on it and closes it, with no other thread inside.
pub(crate) fn with<T>(f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Result<T, arboard::Error> {
    let _turn = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    // Declared after the guard, so it's closed before the lock is released.
    let mut clipboard = arboard::Clipboard::new()?;
    f(&mut clipboard)
}

/// [`with`] on the clipboard kept in `kept`, created on first use, instead of a fresh one. On
/// X11 the copied data is served only while its `Clipboard` lives, so a copy made through a
/// kept clipboard stays pasteable after the call returns.
pub(crate) fn with_kept<T>(
    kept: &mut Option<arboard::Clipboard>,
    f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
) -> Result<T, arboard::Error> {
    let _turn = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let clipboard = match kept {
        Some(clipboard) => clipboard,
        None => kept.insert(arboard::Clipboard::new()?),
    };
    f(clipboard)
}

/// Nonempty text supersedes the app's cached image, even if another clipboard format also exists.
pub(crate) fn has_text() -> bool {
    with(|cb| cb.get_text()).is_ok_and(|text| !text.is_empty())
}

/// The system clipboard as image paste sees it; text takes precedence over a cached image.
pub(crate) fn for_image_paste() -> Option<crate::create_ui::Clip> {
    if has_text() { Some(crate::create_ui::Clip::Text(String::new())) } else { crate::create_ui::read_clipboard() }
}
