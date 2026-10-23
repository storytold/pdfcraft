//! No-op `arboard` stand-in for Android.
//!
//! Android has no `arboard` clipboard backend, so this module stands in with the same surface the
//! UI uses. `Clipboard::new` fails and every operation returns an error, so "paste from the
//! clipboard" and "copy the snapshot to the clipboard" report "unavailable" on Android instead of
//! failing to build.

use std::borrow::Cow;

/// A clipboard error: there is no system clipboard through arboard on Android.
#[derive(Debug)]
pub struct Error;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the system clipboard is not available on Android")
    }
}

impl std::error::Error for Error {}

/// A clipboard image (never produced on Android).
pub struct ImageData<'a> {
    pub width: usize,
    pub height: usize,
    pub bytes: Cow<'a, [u8]>,
}

/// The system clipboard; on Android it can't be opened.
pub struct Clipboard;

impl Clipboard {
    pub fn new() -> Result<Self, Error> {
        Err(Error)
    }

    pub fn get_image(&mut self) -> Result<ImageData<'static>, Error> {
        Err(Error)
    }

    pub fn get_text(&mut self) -> Result<String, Error> {
        Err(Error)
    }

    pub fn set_image(&mut self, _image: ImageData<'_>) -> Result<(), Error> {
        Err(Error)
    }

    /// The builder `arboard::Clipboard::set` returns; every operation fails (no clipboard).
    pub fn set(&mut self) -> Set {
        Set
    }
}

/// Mirrors `arboard::Set`: a chainable setter for clipboard contents.
pub struct Set;

impl Set {
    pub fn file_list(self, _paths: &[impl AsRef<std::path::Path>]) -> Result<(), Error> {
        Err(Error)
    }
}
