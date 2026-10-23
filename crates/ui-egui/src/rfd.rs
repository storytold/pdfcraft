//! No-op `rfd` stand-in for Android.
//!
//! Native file dialogs don't exist on Android (`rfd` has no backend there), so this module stands
//! in for the crate with the same builder and `pick_*` surface the UI already uses. Every pick
//! answers "cancelled", which the UI already treats as "the user closed the dialog", so the editor
//! degrades gracefully instead of failing to build.

use std::future::Future;
use std::path::PathBuf;

/// A native file dialog that never shows: every pick answers "cancelled".
///
/// Mirrors `rfd::AsyncFileDialog`'s builder surface so the UI compiles unchanged.
#[derive(Default)]
pub struct AsyncFileDialog;

impl AsyncFileDialog {
    pub fn new() -> Self {
        Self
    }

    pub fn add_filter(self, _name: impl Into<String>, _extensions: &[impl ToString]) -> Self {
        self
    }

    pub fn set_file_name(self, _file_name: impl Into<String>) -> Self {
        self
    }

    pub fn set_title(self, _title: impl Into<String>) -> Self {
        self
    }

    pub fn pick_file(self) -> impl Future<Output = Option<FileHandle>> {
        std::future::ready(None)
    }

    pub fn pick_files(self) -> impl Future<Output = Option<Vec<FileHandle>>> {
        std::future::ready(None)
    }

    pub fn pick_folder(self) -> impl Future<Output = Option<FileHandle>> {
        std::future::ready(None)
    }

    pub fn save_file(self) -> impl Future<Output = Option<FileHandle>> {
        std::future::ready(None)
    }
}

/// A chosen file; never produced on Android.
pub struct FileHandle;

impl FileHandle {
    pub fn path(&self) -> PathBuf {
        PathBuf::new()
    }
}
