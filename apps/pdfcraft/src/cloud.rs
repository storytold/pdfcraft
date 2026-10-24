//! How this application talks to CollabCraft.
//!
//! [`service`] starts the worker that Open and Save run on. [`client`] builds
//! the one the share dialog uses. Both go through the same service and share
//! one saved session.
//!
//! PdfCraft holds no provider credential and speaks no provider protocol.
//! Everything goes through the CollabCraft service. That service is the one
//! place a provider is configured. This file supplies the transport half of
//! `pdfcraft_ui_egui::cloud`. It also supplies the two things a desktop
//! application has to do that a library cannot: open a browser and decide how
//! long to keep polling.
//!
//! Every call in `craft_cloud_client` blocks. The pages therefore never make
//! one. They state a [`Job`]. This thread runs it. The answer goes back
//! through the [`Post`] the job arrived with. The egui context is woken
//! afterwards. A worker answering is not a local event and the window would
//! otherwise sit still until the mouse moved.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use craft_cloud_client::{Client, Error};
use pdfcraft_ui_egui::cloud::{Account, CloudBackend, Event, Fault, Job, Post};

/// How long to keep polling a sign-in before giving up.
const SIGN_IN_WAIT: Duration = Duration::from_secs(300);

/// Where the CollabCraft service is.
///
/// Read from the environment; nothing is compiled in. The service is
/// deployment-specific. A desktop binary must not carry one. Without it there
/// is no cloud worker and the interface offers nothing cloud-shaped.
fn base_url() -> Option<String> {
    ["PDFCRAFT_CLOUD_URL", "CRAFT_CLOUD_URL", "COLLABCRAFT_URL"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .find(|value| !value.is_empty())
}

/// A handle to the worker, as the pages see it.
///
/// Every job is answered on that thread. Nothing a page does can block a
/// frame. One queue serves them all.
pub struct Service {
    jobs: Sender<(Job, Post)>,
}

impl CloudBackend for Service {
    fn submit(&self, job: Job, post: Post) {
        if self.jobs.send((job, post)).is_err() {
            log::warn!("the cloud worker is gone; the job was dropped");
        }
    }
}

/// Starts the worker, when a service is configured.
pub fn service(ctx: egui::Context) -> Option<Service> {
    let base = base_url()?;
    log::info!("cloud storage through {base}");
    let (jobs, receiver) = std::sync::mpsc::channel::<(Job, Post)>();
    let started = std::thread::Builder::new().name("collabcraft".to_owned()).spawn(move || {
        let token = load_token();
        let mut client = Client::new(&base);
        if let Some(token) = token {
            client = client.with_token(token);
        }
        let mut worker = Worker { client, base, ctx, answering: None };
        while let Ok((job, post)) = receiver.recv() {
            worker.answering = Some(post);
            worker.run(job);
        }
    });
    if let Err(e) = started {
        log::error!("the cloud worker did not start: {e}");
        return None;
    }
    Some(Service { jobs })
}

struct Worker {
    client: Client,
    base: String,
    ctx: egui::Context,
    answering: Option<Post>,
}

impl Worker {
    fn post(&self, event: Event) {
        if let Some(post) = &self.answering {
            post.send(event);
        }
        self.ctx.request_repaint();
    }

    fn run(&mut self, job: Job) {
        match job {
            Job::Account => {
                let answer = self.call(|client| client.me()).map(|me| {
                    remember_account(&me.id, &me.display);
                    Account { display: me.display, capabilities: me.capabilities, provider_linked: me.provider_linked }
                });
                self.post(Event::Account(answer));
            }
            Job::LinkStart => match self.call(|client| client.link_start()) {
                Ok(start) => self.post(Event::Browser { url: start.login_url, handle: Some(start.handle) }),
                Err(fault) => self.post(Event::Linked(Err(fault))),
            },
            Job::Conversation(id) => {
                let answer = self.call(|client| client.conversation(&id));
                self.post(Event::Conversation(answer));
            }
            Job::SignOut => {
                if let Some(path) = token_path() {
                    let _ = std::fs::remove_file(path);
                }
                self.client = Client::new(&self.base);
            }
            Job::LinkPoll(handle) => {
                let answer = self.call(|client| client.link_poll(&handle));
                self.post(Event::Linked(answer));
            }
            Job::List(path) => {
                let entries = self.call(|client| client.list(&path));
                self.post(Event::Listed { path, entries });
            }
            Job::SharedWithMe => {
                let answer = self.call(|client| client.shared_with_me());
                self.post(Event::Shared(answer));
            }
            Job::Read { entry, into } => {
                let bytes = self.call(|client| client.read(&entry.id));
                self.post(Event::Read { entry, into, bytes });
            }
            Job::Write { uid, entry, bytes } => {
                let answer = self.call(|client| client.write(&entry.id, &bytes, entry.etag.as_deref()));
                self.post(Event::Saved { uid, entry: answer });
            }
            Job::Create { uid, folder, name, bytes } => {
                let answer = self.call(|client| client.create(&folder, &name, &bytes));
                self.post(Event::Saved { uid, entry: answer });
            }
        }
    }

    /// Runs one call with a session behind it.
    ///
    /// A rejected token is worth exactly one retry. The saved one has been
    /// revoked or has expired. A fresh sign-in is the only recovery. A second
    /// rejection is reported.
    fn call<T>(&mut self, work: impl Fn(&Client) -> craft_cloud_client::Result<T>) -> Result<T, Fault> {
        self.sign_in()?;
        match work(&self.client) {
            Ok(value) => Ok(value),
            Err(Error::Unauthenticated) => {
                log::info!("the saved cloud session was rejected; signing in again");
                forget_token();
                self.client = Client::new(&self.base);
                self.sign_in()?;
                work(&self.client).map_err(fault)
            }
            Err(e) => Err(fault(e)),
        }
    }

    /// Makes sure there is a session token, signing in through the browser
    /// when there is not.
    fn sign_in(&mut self) -> Result<(), Fault> {
        if self.client.is_signed_in() {
            return Ok(());
        }
        let start = self.client.sign_in_start().map_err(fault)?;
        if let Err(e) = open_browser(&start.verify_url) {
            log::warn!("could not open a browser: {e}");
        }
        self.post(Event::Browser { url: start.verify_url.clone(), handle: None });
        let interval = Duration::from_secs(start.interval.clamp(1, 10));
        let deadline = Instant::now() + SIGN_IN_WAIT.min(Duration::from_secs(start.expires_in));
        while Instant::now() < deadline {
            std::thread::sleep(interval);
            match self.client.sign_in_poll(&start.device_code) {
                Ok(Some(token)) => {
                    save_token(&token);
                    self.client = Client::new(&self.base).with_token(token);
                    self.post(Event::Linked(Ok(true)));
                    return Ok(());
                }
                Ok(None) => {}
                Err(e) => return Err(fault(e)),
            }
        }
        Err(Fault::Refused("the sign-in was not completed in time".to_owned()))
    }
}

/// A client error in the shapes the interface acts on differently.
///
/// A refused conditional write arrives as HTTP 409 with the code `conflict`.
/// The client crate carries that through as an ordinary refusal. The code is
/// read here and given a shape of its own. A save must never retry that one
/// and must never force it.
fn fault(e: Error) -> Fault {
    match e {
        Error::NotLinked => Fault::NotLinked,
        Error::Unauthenticated => Fault::Unauthenticated,
        Error::Unreachable(detail) => Fault::Unreachable(detail),
        Error::Refused { code, .. } if code == "conflict" || code == "409" || code == "412" => Fault::Conflict,
        Error::Refused { code, detail } => Fault::Refused(if detail.trim().is_empty() { code } else { detail }),
        Error::Malformed(detail) => Fault::Refused(format!("the cloud service gave an answer PdfCraft could not read: {detail}")),
        Error::Unsendable(detail) => {
            log::error!("cloud request could not be encoded: {detail}");
            Fault::Refused("PdfCraft could not build that request".to_owned())
        }
    }
}

/// A client for the share dialog, built fresh for each of its requests.
///
/// Separate from the Open and Save worker above because the two answer on
/// different channels. Sharing one client would mean sharing one queue. A
/// large save would then hold up an autocomplete.
pub fn client() -> Option<Client> {
    let mut client = Client::new(base_url()?);
    if let Some(token) = load_token() {
        client = client.with_token(token);
    }
    Some(client)
}

/// Where the session token lives beside the application's own settings.
fn token_path() -> Option<PathBuf> {
    let dir = settings_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("collabcraft-session.json"))
}

fn settings_dir() -> Option<PathBuf> {
    eframe::storage_dir("PdfCraft")
}

fn load_token() -> Option<String> {
    let raw = std::fs::read(token_path()?).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let configured = base_url()?;
    token_for_service(&value, &configured)
}

/// Extracts the token from a saved session only when its stored service URL
/// matches the currently configured one. A URL change means a different
/// deployment, so the token must not be forwarded.
fn token_for_service(value: &serde_json::Value, configured: &str) -> Option<String> {
    let stored = value.get("service")?.as_str()?;
    if stored.trim().trim_end_matches('/') != configured.trim().trim_end_matches('/') {
        log::warn!("the saved cloud session belongs to a different service; ignoring it");
        return None;
    }
    value.get("token")?.as_str().map(str::to_owned).filter(|t| !t.is_empty())
}

fn save_token(token: &str) {
    let service = base_url().unwrap_or_default();
    write_session(serde_json::json!({"token": token, "service": service}));
}

/// Record which account this session belongs to, keeping the token.
fn remember_account(subject: &str, display: &str) {
    let Some(token) = load_token() else { return };
    let service = base_url().unwrap_or_default();
    write_session(serde_json::json!({
        "token": token,
        "service": service,
        "subject": subject,
        "display": display,
    }));
}

fn write_session(value: serde_json::Value) {
    let Some(path) = token_path() else { return };
    let Ok(text) = serde_json::to_vec(&value) else { return };
    if let Err(e) = write_private(&path, &text) {
        log::warn!("could not save the cloud session: {e}");
    }
}

fn forget_token() {
    if let Some(path) = token_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Writes `bytes` to `path` with mode 0o600 on Unix, so that the file is
/// never readable by other users from the instant it is created.
///
/// Opening with an explicit mode avoids the write-then-chmod race window that
/// `std::fs::write` followed by `set_permissions` would create. The second
/// `set_permissions` call tightens any file that a previous version of the app
/// created under a loose umask.
#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    // `mode` applies when the file is first created. Tighten any older file
    // that a previous version of the app may have created under the umask.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)
}

/// On Windows the per-user AppData directory is already owner-scoped, and
/// setting an ACL would need a Win32 call this application makes nowhere else.
#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    // `cmd` splits its command line on `&` before anything else sees it. An
    // authorize URL is mostly `&`. Pass through raw; percent-encode quotes.
    let mut command = {
        use std::os::windows::process::CommandExt as _;
        let mut c = std::process::Command::new("cmd");
        c.raw_arg(format!("/C start \"\" \"{}\"", url.replace('"', "%22")));
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    command.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- token_for_service origin-match tests ---

    fn make_session(service: &str, token: &str) -> serde_json::Value {
        serde_json::json!({"service": service, "token": token})
    }

    #[test]
    fn token_accepted_when_service_matches_exactly() {
        let v = make_session("https://collab.example.com", "tok1");
        assert_eq!(token_for_service(&v, "https://collab.example.com"), Some("tok1".to_owned()));
    }

    #[test]
    fn token_accepted_when_trailing_slash_differs() {
        let v = make_session("https://collab.example.com/", "tok2");
        assert_eq!(token_for_service(&v, "https://collab.example.com"), Some("tok2".to_owned()));
    }

    #[test]
    fn token_rejected_when_service_url_changes() {
        let v = make_session("https://collab.example.com", "tok3");
        assert_eq!(token_for_service(&v, "https://other.example.com"), None);
    }

    #[test]
    fn token_rejected_when_service_field_absent() {
        // A file written by an older version that only stored "token".
        let v = serde_json::json!({"token": "tok4"});
        assert_eq!(token_for_service(&v, "https://collab.example.com"), None);
    }

    #[test]
    fn empty_token_is_treated_as_absent() {
        let v = make_session("https://collab.example.com", "");
        assert_eq!(token_for_service(&v, "https://collab.example.com"), None);
    }

    // --- write_private mode-0600 test (Unix only) ---

    #[test]
    #[cfg(unix)]
    fn write_private_creates_file_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = std::env::temp_dir().join(format!("pdfcraft-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");

        write_private(&path, b"{\"test\":true}").expect("write_private failed");

        let meta = std::fs::metadata(&path).expect("metadata failed");
        let mode = meta.permissions().mode() & 0o777;
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(mode, 0o600, "expected mode 0600, got {mode:o}");
    }

    #[test]
    #[cfg(unix)]
    fn write_private_tightens_existing_file_with_loose_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = std::env::temp_dir().join(format!("pdfcraft-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");

        // Pre-create with loose permissions, as an older app version might have done.
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_private(&path, b"{\"test\":true}").expect("write_private failed");

        let meta = std::fs::metadata(&path).expect("metadata failed");
        let mode = meta.permissions().mode() & 0o777;
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(mode, 0o600, "expected mode 0600 after tightening, got {mode:o}");
    }
}
