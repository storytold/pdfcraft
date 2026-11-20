/// Files delivered asynchronously: (name, bytes).
pub type Inbox = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// Files that failed to arrive asynchronously (a browser `?file=` URL that couldn't be fetched):
/// `(name, error)`, reported to the user on the next frame (#173).
pub type FailedInbox = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// A request the operating system sends the running app, outside its window: on macOS, Finder
/// double-clicks, Open With and drops on the Dock icon arrive as Apple events, not arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OsEvent {
    /// Open these files.
    Open(Vec<String>),
    /// Add these files to Combine files and show it (`pdfcraft --combine`, File Explorer's
    /// "Combine files in PdfCraft", #585).
    Combine(Vec<String>),
    /// Quit (the Dock's Quit, logging out), asking about unsaved changes first.
    Quit,
}

/// Returns the [`OsEvent`]s that arrived since it was last called (set by the desktop app).
pub type OsEventsFn = Box<dyn FnMut() -> Vec<OsEvent>>;

/// Hands a saved PDF to the page embedding PdfCraft (web): `(name, bytes, save_as)`. `save_as`
/// is `true` for Save As. Set by the web app when a host page (such as a Nextcloud app) asked
/// for saves; without it the browser downloads the file.
pub type HostSaveFn = Box<dyn Fn(&str, &[u8], bool) -> Result<(), String>>;

/// Told every frame whether any tab has unsaved work, or an OCR review is not yet accepted or
/// rejected, so a host page can warn before leaving.
pub type HostDirtyFn = Box<dyn FnMut(bool)>;

/// A collaboration session the host page handed over (`pdfcraft:session`): the
/// socket, the credential, and the role the cabinet believes it assigned. The
/// token decides the real role (ADR-0008 §7); the server's `welcome` is what the
/// app shows.
#[derive(Clone, PartialEq, Eq)]
pub struct HostSession {
    pub session_id: String,
    pub url: String,
    /// A credential: never logged, never put in a URL or a DOM attribute.
    pub token: String,
    pub role: Option<String>,
}

impl std::fmt::Debug for HostSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostSession")
            .field("session_id", &self.session_id)
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .field("role", &self.role)
            .finish()
    }
}

/// Sessions the host page handed over, taken once per frame.
pub type HostSessionInbox = std::sync::Arc<std::sync::Mutex<Vec<HostSession>>>;

/// The cabinet's answers to a commit (`pdfcraft:committed`): `(version, etag)`.
pub type CommittedInbox = std::sync::Arc<std::sync::Mutex<Vec<(u64, String)>>>;
