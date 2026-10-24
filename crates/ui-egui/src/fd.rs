//! Where the file dialogs come from.
//!
//! Desktop builds ask `rfd`, which has a backend for macOS, Windows, Linux and the web. Android
//! has none: `rfd` 0.17 does not compile for it at all (its platform traits have no
//! implementation there), and an Android picker must be started on the UI thread and answered
//! later — which a blocking dialog can never do, because the frame that would receive the answer
//! is the frame it blocks.
//!
//! So every dialog in this crate asks this module, never `rfd` directly:
//!
//! - on the platforms `rfd` supports the two types *are* `rfd`'s, unchanged;
//! - on Android a host installs itself with [`set_host`] (the app shell does: see
//!   `apps/printcraft-android`, which drives the Storage Access Framework). The synchronous
//!   dialogs then answer immediately — a *save* reserves a file in the app's own storage and the
//!   host publishes it to the destination the user picks afterwards; an *open* answers
//!   "cancelled", because a pick can only come back on a later frame (File ▸ Open uses the
//!   asynchronous dialog, which exists for exactly that reason);
//! - with no host installed every dialog answers "cancelled": the app keeps running and can open
//!   files by path, it just cannot ask for one.

#[cfg(not(target_os = "android"))]
pub use rfd::{AsyncFileDialog, FileDialog, FileHandle};

#[cfg(target_os = "android")]
mod android {
    use std::future::Future;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::{Arc, OnceLock};

    /// What a dialog is asking for (the builder's state, in `rfd`'s own terms).
    #[derive(Clone, Debug, Default)]
    pub struct DialogRequest {
        /// The dialog's title, when one was set.
        pub title: String,
        /// The suggested file name (save dialogs) or `""`.
        pub file_name: String,
        /// `(name, extensions)` pairs added with `add_filter`.
        pub filters: Vec<(String, Vec<String>)>,
        /// The folder a `set_directory` asked to start in, when one was set.
        pub directory: Option<PathBuf>,
    }

    /// The platform's pickers. Installed once by the app shell; called from the UI thread (the
    /// synchronous dialogs) and from worker threads (the asynchronous ones), so it must be
    /// `Send + Sync`.
    pub trait Host: Send + Sync + 'static {
        /// Show the open picker and wait for it. Returns the picked files' real paths — the host
        /// copies them into the app's own storage, because the rest of the app reads paths — or
        /// an empty list when the user cancels.
        fn open(&self, request: &DialogRequest, multiple: bool) -> Vec<PathBuf>;

        /// A path a save may be written to. The host asks the user where the finished file should
        /// go (and copies it there once it is written); `None` when the user declines to choose.
        fn save(&self, request: &DialogRequest) -> Option<PathBuf>;

        /// A folder the app may write files into: exports, split results, OCR output.
        fn folder(&self) -> Option<PathBuf>;
    }

    static HOST: OnceLock<Arc<dyn Host>> = OnceLock::new();

    /// Install the platform's pickers. Call it before the window opens; a second call is ignored.
    pub fn set_host(host: Arc<dyn Host>) {
        if HOST.set(host).is_err() {
            log::warn!("fd: a dialog host was already installed");
        }
    }

    /// Whether a host is installed (the shell logs it at start-up).
    pub fn host_installed() -> bool {
        HOST.get().is_some()
    }

    fn host() -> Option<&'static Arc<dyn Host>> {
        HOST.get()
    }

    /// The future an asynchronous dialog returns (rfd's `DialogFutureType`).
    pub type DialogFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

    /// A file the picker handed back.
    #[derive(Clone, Debug)]
    pub struct FileHandle {
        path: PathBuf,
    }

    impl FileHandle {
        pub fn path(&self) -> &std::path::Path {
            &self.path
        }

        pub fn file_name(&self) -> String {
            self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        }
    }

    /// A blocking file dialog.
    ///
    /// The synchronous dialogs must answer *inside* the frame that asks (the call sites use the
    /// result immediately), so an open cannot be answered here at all — it reports "cancelled"
    /// and logs that the asynchronous dialog is the one that works on this platform. A save can:
    /// the host reserves a file in the app's own storage and publishes the finished file later.
    #[derive(Clone, Debug, Default)]
    pub struct FileDialog {
        request: DialogRequest,
    }

    impl FileDialog {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn add_filter(mut self, name: impl Into<String>, extensions: &[impl AsRef<str>]) -> Self {
            self.request.filters.push((name.into(), extensions.iter().map(|e| e.as_ref().to_owned()).collect()));
            self
        }

        pub fn set_title(mut self, title: impl Into<String>) -> Self {
            self.request.title = title.into();
            self
        }

        pub fn set_file_name(mut self, name: impl Into<String>) -> Self {
            self.request.file_name = name.into();
            self
        }

        /// Kept for parity with `rfd`'s builder. The Android host decides the folder (the app's
        /// own storage), so the suggestion is recorded but not acted on here.
        pub fn set_directory(mut self, dir: impl AsRef<std::path::Path>) -> Self {
            self.request.directory = Some(dir.as_ref().to_path_buf());
            self
        }

        /// A pick cannot be answered in this frame: see the type's docs and [`AsyncFileDialog`].
        pub fn pick_file(self) -> Option<PathBuf> {
            log::info!("fd: FileDialog::pick_file needs the asynchronous dialog on this platform; answering \"cancelled\"");
            None
        }

        /// A pick cannot be answered in this frame: see the type's docs and [`AsyncFileDialog`].
        pub fn pick_files(self) -> Option<Vec<PathBuf>> {
            log::info!("fd: FileDialog::pick_files needs the asynchronous dialog on this platform; answering \"cancelled\"");
            None
        }

        /// Folders the app may write into (the host's own directory).
        pub fn pick_folder(self) -> Option<PathBuf> {
            host().and_then(|h| h.folder())
        }

        /// A save destination: the host reserves a scratch file now and publishes the finished
        /// file to wherever the user points.
        pub fn save_file(self) -> Option<PathBuf> {
            host().and_then(|h| h.save(&self.request))
        }
    }

    /// A dialog that answers on a later frame, polled on a worker thread (which is what lets the
    /// Android picker work: the UI thread is free to deliver the result).
    #[derive(Clone, Debug, Default)]
    pub struct AsyncFileDialog {
        request: DialogRequest,
    }

    impl AsyncFileDialog {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn add_filter(mut self, name: impl Into<String>, extensions: &[impl AsRef<str>]) -> Self {
            self.request.filters.push((name.into(), extensions.iter().map(|e| e.as_ref().to_owned()).collect()));
            self
        }

        pub fn set_title(mut self, title: impl Into<String>) -> Self {
            self.request.title = title.into();
            self
        }

        pub fn set_file_name(mut self, name: impl Into<String>) -> Self {
            self.request.file_name = name.into();
            self
        }

        /// Kept for parity with `rfd`'s builder: Android's pickers do not start in a suggested
        /// directory (the host chooses where its own pickers start).
        pub fn set_directory(mut self, dir: impl AsRef<std::path::Path>) -> Self {
            self.request.directory = Some(dir.as_ref().to_path_buf());
            self
        }

        /// A folder the app may write files into (exports, split results, OCR output).
        pub fn pick_folder(self) -> DialogFuture<Option<FileHandle>> {
            Box::pin(async move { host()?.folder().map(|path| FileHandle { path }) })
        }

        pub fn pick_file(self) -> DialogFuture<Option<FileHandle>> {
            let request = self.request;
            Box::pin(async move { host()?.open(&request, false).into_iter().next().map(|path| FileHandle { path }) })
        }

        pub fn pick_files(self) -> DialogFuture<Option<Vec<FileHandle>>> {
            let request = self.request;
            Box::pin(async move {
                let paths = host()?.open(&request, true);
                if paths.is_empty() { None } else { Some(paths.into_iter().map(|path| FileHandle { path }).collect()) }
            })
        }

        pub fn save_file(self) -> DialogFuture<Option<FileHandle>> {
            let request = self.request;
            Box::pin(async move { host()?.save(&request).map(|path| FileHandle { path }) })
        }
    }
}

#[cfg(target_os = "android")]
pub use android::{AsyncFileDialog, DialogFuture, DialogRequest, FileDialog, FileHandle, Host, host_installed, set_host};
