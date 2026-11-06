//! Native clipboard access: serialize backend lifetimes, but never wait for the lock.
//! Reads run on a worker; a selection owner may never answer (including ourselves on Wayland).

use std::sync::{Mutex, MutexGuard, TryLockError, mpsc};
use std::time::Duration;

use crate::create_ui::{Clip, MAX_IMAGE_BYTES, MAX_IMAGE_SIDE};

static LOCK: Mutex<()> = Mutex::new(());
// Windows/macOS publish our clipboard data eagerly. Allow headroom above the Windows
// review's 1.07 s image read; keep selection-owner waits short on Linux/BSD (Wayland/X11).
// External owners can still stall on any platform, so neither budget is unbounded.
const READ_TIMEOUT: Duration = read_timeout_for(cfg!(any(target_os = "windows", target_os = "macos")));

const fn read_timeout_for(windows_or_macos: bool) -> Duration {
    Duration::from_secs(if windows_or_macos { 5 } else { 1 })
}

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
        // Do not request the lower-priority format when the preferred one is usable.
        let (image, text) = match mode {
            ReadMode::ImagePaste => {
                let text = cb.get_text();
                let image = if text.as_ref().is_ok_and(|text| !text.is_empty()) { Err(arboard::Error::ContentNotAvailable) } else { cb.get_image() };
                (image, text)
            }
            ReadMode::Create => {
                let image = cb.get_image();
                let text = if image.as_ref().is_ok_and(valid_image) { Err(arboard::Error::ContentNotAvailable) } else { cb.get_text() };
                (image, text)
            }
        };
        Ok(select_content(image, text, mode))
    })
}

fn valid_image(image: &arboard::ImageData<'_>) -> bool {
    let expected = image.width.checked_mul(image.height).and_then(|px| px.checked_mul(4));
    image.width > 0
        && image.height > 0
        && image.width <= MAX_IMAGE_SIDE
        && image.height <= MAX_IMAGE_SIDE
        && expected == Some(image.bytes.len())
        && image.bytes.len() <= MAX_IMAGE_BYTES
}

/// Pure format policy, shared by both native read modes. An unavailable result may also mean
/// that the preferred format was usable, so the other format was never requested.
fn select_content(image: Result<arboard::ImageData<'_>, arboard::Error>, text: Result<String, arboard::Error>, mode: ReadMode) -> Option<Clip> {
    if matches!(mode, ReadMode::ImagePaste) && text.as_ref().is_ok_and(|text| !text.is_empty()) {
        return Some(Clip::Text(String::new()));
    }
    if let Ok(image) = image
        && valid_image(&image)
    {
        return Some(Clip::Image { width: image.width, height: image.height, rgba: image.bytes.into_owned() });
    }
    text.ok().filter(|text| !text.trim().is_empty()).map(Clip::Text)
}

pub(crate) fn for_image_paste() -> Result<Option<Clip>, NotResponding> {
    read(ReadMode::ImagePaste)
}

#[cfg(test)]
mod tests;
