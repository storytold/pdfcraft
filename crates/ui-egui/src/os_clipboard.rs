//! The one way to the operating system's clipboard.
//!
//! The system clipboard is a single process-wide resource: on Windows, two threads that
//! open it at once can corrupt the heap. Every access goes through [`with`], which holds a
//! lock across the whole open, use and close, so parallel callers (test threads, workers)
//! take turns instead of racing.

use std::sync::{Mutex, PoisonError};

static LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    /// Test hook: how many times this thread has opened the system clipboard.
    static OPENS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Test hook: this thread's writes ([`PdfCraftApp::put_on_system_clipboard`]) succeed
    /// without touching the system clipboard.
    ///
    /// [`PdfCraftApp::put_on_system_clipboard`]: crate::PdfCraftApp::put_on_system_clipboard
    static FAKE_WRITES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Makes this thread's clipboard writes succeed without touching the system clipboard.
#[cfg(test)]
pub(crate) fn fake_writes() {
    FAKE_WRITES.with(|fake| fake.set(true));
}

/// Whether this thread's clipboard writes are faked ([`fake_writes`]).
#[cfg(test)]
pub(crate) fn writes_faked() -> bool {
    FAKE_WRITES.with(std::cell::Cell::get)
}

/// How many times this thread has opened the system clipboard ([`with`]).
#[cfg(test)]
pub(crate) fn opens() -> usize {
    OPENS.with(std::cell::Cell::get)
}

/// Opens the system clipboard, runs `f` on it and closes it, with no other thread inside.
pub(crate) fn with<T>(f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Result<T, arboard::Error> {
    #[cfg(test)]
    OPENS.with(|n| n.set(n.get() + 1));
    let _turn = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    // Declared after the guard, so it's closed before the lock is released.
    let mut clipboard = arboard::Clipboard::new()?;
    f(&mut clipboard)
}

/// Nonempty text supersedes the app's cached image, even if another clipboard format also exists.
pub(crate) fn has_text() -> bool {
    with(|cb| cb.get_text()).is_ok_and(|text| !text.is_empty())
}

/// The system clipboard as image paste sees it; text takes precedence over a cached image.
pub(crate) fn for_image_paste() -> Option<crate::create_ui::Clip> {
    if has_text() { Some(crate::create_ui::Clip::Text(String::new())) } else { crate::create_ui::read_clipboard() }
}
