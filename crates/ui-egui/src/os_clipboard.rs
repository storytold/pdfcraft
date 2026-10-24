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
