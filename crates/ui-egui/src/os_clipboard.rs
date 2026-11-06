//! Native clipboard access: serialize backend lifetimes, but never wait for the lock.
//! Reads run on a worker; a selection owner may never answer (including ourselves on Wayland).

use std::sync::{Mutex, MutexGuard, TryLockError, mpsc};
use std::time::Duration;

use crate::create_ui::{Clip, MAX_IMAGE_BYTES, MAX_IMAGE_SIDE};

static LOCK: Mutex<()> = Mutex::new(());
const READ_TIMEOUT: Duration = Duration::from_secs(1);

fn acquire(lock: &Mutex<()>) -> Result<MutexGuard<'_, ()>, arboard::Error> {
    match lock.try_lock() {
        Ok(guard) => Ok(guard),
        Err(TryLockError::Poisoned(error)) => Ok(error.into_inner()),
        Err(TryLockError::WouldBlock) => Err(arboard::Error::ClipboardOccupied),
    }
}

/// No two backends may be open at once (notably on Windows). A busy backend fails promptly,
/// so a detached, stuck reader cannot make a later Copy wait on its lock.
pub(crate) fn with<T>(f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Result<T, arboard::Error> {
    #[cfg(test)]
    if let Some(result) = tests::access() {
        result?;
        return Err(arboard::Error::ClipboardNotSupported);
    }
    let _turn = acquire(&LOCK)?;
    // Closed before the guard releases the lock.
    let mut clipboard = arboard::Clipboard::new()?;
    f(&mut clipboard)
}

#[derive(Clone, Copy)]
pub(crate) enum ReadMode {
    /// Text supersedes images and stale image caches on Paste.
    ImagePaste,
    /// Create prefers an image, then accepts text.
    Create,
}

/// A timeout or occupied backend; neither may turn into a stale-cache paste.
#[derive(Debug)]
pub(crate) struct NotResponding;

/// Bound the UI's wait, not the backend's uninterruptible pipe read. On timeout the worker
/// is detached; its lock still excludes another native backend, without blocking callers.
pub(crate) fn read(mode: ReadMode) -> Result<Option<Clip>, NotResponding> {
    let (send, receive) = mpsc::sync_channel(1);
    #[cfg(test)]
    let backend = tests::backend();
    let worker = std::thread::Builder::new().name("clipboard-read".into()).spawn(move || {
        #[cfg(test)]
        let _probe = tests::install(backend);
        let _ = send.send(read_native(mode));
    });
    if worker.is_err() {
        return Ok(None);
    }
    match receive.recv_timeout(READ_TIMEOUT) {
        Ok(Err(arboard::Error::ClipboardOccupied)) | Err(mpsc::RecvTimeoutError::Timeout) => Err(NotResponding),
        Ok(result) => Ok(result.ok().flatten()),
        Err(mpsc::RecvTimeoutError::Disconnected) => Ok(None),
    }
}

fn read_native(mode: ReadMode) -> Result<Option<Clip>, arboard::Error> {
    with(|cb| {
        if matches!(mode, ReadMode::ImagePaste) && cb.get_text().is_ok_and(|text| !text.is_empty()) {
            return Ok(Some(Clip::Text(String::new())));
        }
        if let Ok(img) = cb.get_image() {
            let expected = img.width.checked_mul(img.height).and_then(|px| px.checked_mul(4));
            if img.width > 0
                && img.height > 0
                && img.width <= MAX_IMAGE_SIDE
                && img.height <= MAX_IMAGE_SIDE
                && expected == Some(img.bytes.len())
                && img.bytes.len() <= MAX_IMAGE_BYTES
            {
                return Ok(Some(Clip::Image { width: img.width, height: img.height, rgba: img.bytes.into_owned() }));
            }
            return Ok(None);
        }
        Ok(cb.get_text().ok().filter(|text| !text.trim().is_empty()).map(Clip::Text))
    })
}

pub(crate) fn for_image_paste() -> Result<Option<Clip>, NotResponding> {
    read(ReadMode::ImagePaste)
}

#[cfg(test)]
mod tests;
