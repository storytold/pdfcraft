//! Linux Print portal transport. A finished PDF crosses the sandbox boundary as a read-only
//! descriptor, never a host pathname. Run these blocking entry points on a worker.
//! Protocol: <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Print.html>.

use crate::PrintError;
use futures_lite::{StreamExt, future};
use std::{
    collections::HashMap,
    fs::File,
    future::Future,
    io::Write,
    os::fd::{AsFd, AsRawFd},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use zbus::{
    Connection, Proxy,
    zvariant::{Fd, OwnedObjectPath, OwnedValue, Value},
};

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.Print";

/// Acceptance by the portal is not confirmation of physical printing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    Cancelled,
}

/// Dropping the UI's job cancels its request, including during application shutdown.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

fn error(message: impl std::fmt::Display) -> PrintError {
    PrintError::Spool(format!(
        "System printing failed: {message}. Check that xdg-desktop-portal and your desktop's print portal are running, or use Save as PDF."
    ))
}

/// Start with an anonymous file: even shutdown during a long write cannot leave a named PDF.
/// Reopening our own descriptor gives an independent, read-only file description at offset zero;
/// duplicating the writable descriptor would preserve its access mode and end-of-file offset.
fn document(pdf: &[u8]) -> Result<File, PrintError> {
    let mut temp = tempfile::tempfile().map_err(error)?;
    temp.write_all(pdf).map_err(error)?;
    File::open(format!("/proc/self/fd/{}", temp.as_raw_fd())).map_err(error)
}

/// Dependency injection boundary: tests and other callers can supply a transport without
/// opening a real dialog, while exercising the same document handoff and ownership.
pub trait Transport {
    fn print(&self, file: &File, title: &str, parent: &str, cancel: &Cancellation) -> Result<Outcome, PrintError>;
}

pub fn submit_with(transport: &dyn Transport, pdf: &[u8], title: &str, parent: &str, cancel: &Cancellation) -> Result<Outcome, PrintError> {
    if cancel.is_cancelled() {
        return Ok(Outcome::Cancelled);
    }
    let file = document(pdf)?;
    transport.print(&file, title, parent, cancel)
}

pub fn submit(pdf: &[u8], title: &str, parent: &str, cancel: &Cancellation) -> Result<Outcome, PrintError> {
    submit_with(&Desktop, pdf, title, parent, cancel)
}

struct Desktop;
impl Transport for Desktop {
    fn print(&self, file: &File, title: &str, parent: &str, cancel: &Cancellation) -> Result<Outcome, PrintError> {
        future::block_on(async {
            let connection = bounded(
                async { zbus::connection::Builder::session().map_err(error)?.method_timeout(Duration::from_secs(30)).build().await.map_err(error) },
                Duration::from_secs(30),
                cancel,
            )
            .await?;
            let Some(connection) = connection else {
                return Ok(Outcome::Cancelled);
            };
            print_on(&connection, DESTINATION, file, title, parent, cancel, Duration::from_secs(15 * 60)).await
        })
    }
}

/// Bounded waits yield to the executor; cancellation never blocks the frame loop.
async fn bounded<T>(work: impl Future<Output = Result<T, PrintError>>, timeout: Duration, cancel: &Cancellation) -> Result<Option<T>, PrintError> {
    if cancel.is_cancelled() {
        return Ok(None);
    }
    future::or(async { work.await.map(Some) }, async {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            if std::time::Instant::now() >= deadline {
                return Err(error("the portal did not respond in time; check the system print queue before retrying"));
            }
            async_io::Timer::after(Duration::from_millis(50)).await;
        }
    })
    .await
}

enum Operation<'a> {
    Prepare,
    Print { file: &'a File, token: u32 },
}

async fn print_on(
    connection: &Connection,
    destination: &str,
    file: &File,
    title: &str,
    parent: &str,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<Outcome, PrintError> {
    let (outcome, mut values) = request_on(connection, destination, Operation::Prepare, title, parent, cancel, timeout).await?;
    if outcome == Outcome::Cancelled {
        return Ok(outcome);
    }
    let token = values.remove("token").ok_or_else(|| error("PreparePrint returned no token"))?.try_into().map_err(error)?;
    let settings: HashMap<String, OwnedValue> =
        values.remove("settings").ok_or_else(|| error("PreparePrint returned no settings"))?.try_into().map_err(error)?;
    let _page_setup: HashMap<String, OwnedValue> =
        values.remove("page-setup").ok_or_else(|| error("PreparePrint returned no page setup"))?.try_into().map_err(error)?;
    // Composition already happened in PdfCraft. Never apply host page selection, imposition
    // or scaling a second time, nor silently ignore a conflicting choice in the host dialog.
    for (key, expected) in [("print-pages", "all"), ("page-set", "all"), ("number-up", "1")] {
        if let Some(value) = settings.get(key) {
            let value = <&str>::try_from(value).map_err(error)?;
            if value != expected {
                return Err(PrintError::Invalid("The PDF is already laid out: choose All pages and one page per sheet in the system dialog; set page selection and layout in PdfCraft".into()));
            }
        }
    }
    if let Some(reverse) = settings.get("reverse") {
        // KDE's observed PreparePrint response uses yes/no; GTK uses true/false.
        // Both spellings of false preserve the composed sheet order.
        if !matches!(<&str>::try_from(reverse).map_err(error)?, "false" | "no") {
            return Err(PrintError::Invalid("The PDF is already ordered: turn off reverse order in the system dialog".into()));
        }
    }
    if let Some(format) = settings.get("output-file-format")
        && !<&str>::try_from(format).map_err(error)?.eq_ignore_ascii_case("pdf")
    {
        return Err(PrintError::Invalid("PdfCraft supplies PDF output: choose PDF in the system print dialog".into()));
    }
    if let Some(scale) = settings.get("scale") {
        let scale = <&str>::try_from(scale).map_err(error)?.parse::<f64>().map_err(error)?;
        if scale != 100.0 {
            return Err(PrintError::Invalid("The PDF is already scaled: choose 100% scale in the system dialog".into()));
        }
    }
    request_on(connection, destination, Operation::Print { file, token }, title, parent, cancel, timeout).await.map(|(outcome, _)| outcome)
}

async fn request_on(
    connection: &Connection,
    destination: &str,
    operation: Operation<'_>,
    title: &str,
    parent: &str,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<(Outcome, HashMap<String, OwnedValue>), PrintError> {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sender =
        connection.unique_name().ok_or_else(|| error("D-Bus supplied no unique connection name"))?.as_str().trim_start_matches(':').replace('.', "_");
    let token = format!("pdfcraft_{}", SERIAL.fetch_add(1, Ordering::Relaxed));
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    // Subscribe before either method: a backend may respond before returning its method reply.
    let request = Proxy::new(connection, destination, path.as_str(), "org.freedesktop.portal.Request").await.map_err(error)?;
    let work = async {
        let mut responses = request.receive_signal("Response").await.map_err(error)?;
        let portal = Proxy::new(connection, destination, PATH, INTERFACE).await.map_err(error)?;
        let version: u32 = portal.get_property("version").await.map_err(error)?;
        if version == 0 {
            return Err(error("the Print interface is unsupported"));
        }
        let mut owners = portal.receive_owner_changed().await.map_err(error)?;
        let mut options: HashMap<&str, Value<'_>> =
            HashMap::from([("handle_token", Value::from(token.as_str())), ("modal", Value::from(!parent.is_empty()))]);
        if version >= 3 {
            options.insert("supported_output_file_formats", Value::from(vec!["pdf"]));
        }
        let handle: OwnedObjectPath = match operation {
            Operation::Prepare => {
                let settings: HashMap<&str, Value<'_>> = [
                    ("print-pages", "all"),
                    ("page-set", "all"),
                    ("number-up", "1"),
                    ("scale", "100"),
                    ("reverse", "false"),
                    ("output-file-format", "pdf"),
                ]
                .into_iter()
                .map(|(k, v)| (k, Value::from(v)))
                .collect();
                let page_setup = HashMap::<&str, Value<'_>>::new();
                if version >= 4 {
                    options.insert("has_current_page", Value::from(false));
                    options.insert("has_selected_pages", Value::from(false));
                }
                portal.call("PreparePrint", &(parent, title, settings, page_setup, options)).await.map_err(error)?
            }
            Operation::Print { file, token } => {
                options.insert("token", Value::from(token));
                portal.call("Print", &(parent, title, Fd::from(file.as_fd()), options)).await.map_err(error)?
            }
        };
        if handle.as_str() != path {
            return Err(error("the portal returned an unexpected request handle"));
        }
        let message = future::or(async { responses.next().await.ok_or_else(|| error("the portal disconnected before responding")) }, async {
            let _ = owners.next().await;
            Err(error("the print portal became unavailable before responding"))
        })
        .await?;
        let (status, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize().map_err(error)?;
        match status {
            0 => Ok((Outcome::Accepted, results)),
            1 => Ok((Outcome::Cancelled, results)),
            2 => Err(error("the portal could not print the document")),
            _ => Err(error("the portal returned an invalid response code")),
        }
    };
    let result = bounded(work, timeout, cancel).await;
    if !matches!(result, Ok(Some((Outcome::Accepted | Outcome::Cancelled, _)))) {
        // Best effort, bounded even when the service has disappeared. Drop the connection and
        // all signal matches afterwards; no detached task or descriptor survives this request.
        let _ = future::or(
            async {
                request.call::<_, _, ()>("Close", &()).await.ok();
            },
            async {
                async_io::Timer::after(Duration::from_secs(2)).await;
            },
        )
        .await;
    }
    result.map(|value| value.unwrap_or((Outcome::Cancelled, HashMap::new())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read},
        os::unix::fs::MetadataExt,
        process::{Child, Command, Stdio},
    };
    use zbus::zvariant::OwnedFd;

    struct Bus(Child, String);
    impl Bus {
        fn new() -> Self {
            let mut child = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).spawn().unwrap();
            let mut address = String::new();
            BufReader::new(child.stdout.take().unwrap()).read_line(&mut address).unwrap();
            Self(child, address.trim().into())
        }
        async fn connect(&self) -> Connection {
            zbus::connection::Builder::address(self.1.as_str()).unwrap().build().await.unwrap()
        }
    }
    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum Mode {
        Response(u32),
        Malformed,
        Pending,
        HandoffFailed,
        Shutdown,
        Disconnect,
        WrongHandle,
        PrepareCancelled,
        MissingToken,
        InvalidToken,
        ConflictingLayout,
        UnsupportedOutput,
    }
    struct Mock {
        mode: Mode,
        cancel: Cancellation,
        expected: Vec<u8>,
        handoffs: Arc<std::sync::atomic::AtomicUsize>,
        closed: Arc<AtomicBool>,
    }
    struct Request(Arc<AtomicBool>);
    #[zbus::interface(name = "org.freedesktop.portal.Request")]
    impl Request {
        fn close(&self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    #[zbus::interface(name = "org.freedesktop.portal.Print")]
    impl Mock {
        #[zbus(property, name = "version")]
        fn version(&self) -> u32 {
            4
        }
        #[allow(clippy::too_many_arguments)] // The portal D-Bus signature fixes these parameters.
        async fn prepare_print(
            &self,
            parent: &str,
            title: &str,
            settings: HashMap<String, OwnedValue>,
            _page_setup: HashMap<String, OwnedValue>,
            options: HashMap<String, OwnedValue>,
            #[zbus(connection)] connection: &Connection,
            #[zbus(header)] header: zbus::message::Header<'_>,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            assert_eq!(parent, "");
            assert_eq!(title, "synthetic.pdf");
            assert_eq!(<&str>::try_from(settings.get("number-up").unwrap()).unwrap(), "1");
            assert_eq!(<&str>::try_from(settings.get("scale").unwrap()).unwrap(), "100");
            let mut settings = settings;
            settings.insert("reverse".into(), OwnedValue::try_from(Value::from("no")).unwrap());
            let sender = header.sender().unwrap();
            let token = <&str>::try_from(options.get("handle_token").unwrap()).unwrap();
            let path = OwnedObjectPath::try_from(format!(
                "/org/freedesktop/portal/desktop/request/{}/{}",
                sender.as_str().trim_start_matches(':').replace('.', "_"),
                token
            ))
            .unwrap();
            let mut values: HashMap<&str, OwnedValue> = HashMap::from([
                ("token", OwnedValue::from(17u32)),
                ("settings", OwnedValue::try_from(Value::from(settings)).unwrap()),
                ("page-setup", OwnedValue::try_from(Value::from(HashMap::<String, OwnedValue>::new())).unwrap()),
            ]);
            if matches!(self.mode, Mode::PrepareCancelled) {
                connection
                    .emit_signal(
                        Some(sender.as_str()),
                        path.clone(),
                        "org.freedesktop.portal.Request",
                        "Response",
                        &(1u32, HashMap::<String, OwnedValue>::new()),
                    )
                    .await
                    .unwrap();
                return Ok(path);
            }
            if matches!(self.mode, Mode::MissingToken) {
                values.remove("token");
            }
            if matches!(self.mode, Mode::InvalidToken) {
                values.insert("token", OwnedValue::try_from(Value::from("invalid")).unwrap());
            }
            if matches!(self.mode, Mode::UnsupportedOutput) {
                let unsupported: HashMap<&str, Value<'_>> = HashMap::from([("output-file-format", Value::from("ps"))]);
                values.insert("settings", OwnedValue::try_from(Value::from(unsupported)).unwrap());
            }
            if matches!(self.mode, Mode::ConflictingLayout) {
                let conflict: HashMap<&str, Value<'_>> = HashMap::from([("number-up", Value::from("4"))]);
                values.insert("settings", OwnedValue::try_from(Value::from(conflict)).unwrap());
            }
            connection.emit_signal(Some(sender.as_str()), path.clone(), "org.freedesktop.portal.Request", "Response", &(0u32, values)).await.unwrap();
            Ok(path)
        }

        async fn print(
            &self,
            parent: &str,
            title: &str,
            fd: OwnedFd,
            options: HashMap<String, OwnedValue>,
            #[zbus(connection)] connection: &Connection,
            #[zbus(header)] header: zbus::message::Header<'_>,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            assert!(
                !matches!(
                    self.mode,
                    Mode::PrepareCancelled | Mode::MissingToken | Mode::InvalidToken | Mode::ConflictingLayout | Mode::UnsupportedOutput
                ),
                "failed PreparePrint must never hand off the PDF"
            );
            self.handoffs.fetch_add(1, Ordering::Relaxed);
            assert_eq!(parent, "");
            assert_eq!(title, "synthetic.pdf");
            assert_eq!(u32::try_from(options.get("token").unwrap()).unwrap(), 17);
            let formats: Vec<String> = options.get("supported_output_file_formats").unwrap().try_clone().unwrap().try_into().unwrap();
            assert_eq!(formats, ["pdf"]);
            let mut file = File::from(std::os::fd::OwnedFd::from(fd));
            let mut actual = Vec::new();
            file.read_to_end(&mut actual).unwrap();
            assert_eq!(actual, self.expected, "no page selection, scaling or N-up is applied by the transport");
            assert!(file.write_all(b"must be read-only").is_err());
            if matches!(self.mode, Mode::HandoffFailed) {
                return Err(zbus::fdo::Error::Failed("handoff failed".into()));
            }
            let sender = header.sender().unwrap();
            let token = <&str>::try_from(options.get("handle_token").unwrap()).unwrap();
            let path = OwnedObjectPath::try_from(format!(
                "/org/freedesktop/portal/desktop/request/{}/{}",
                sender.as_str().trim_start_matches(':').replace('.', "_"),
                token
            ))
            .unwrap();
            connection.object_server().at(path.clone(), Request(self.closed.clone())).await.unwrap();
            if matches!(self.mode, Mode::Shutdown) {
                self.cancel.cancel();
            }
            if matches!(self.mode, Mode::Disconnect) {
                connection.release_name("org.example.PdfCraftPrintTest").await.unwrap();
            }
            if matches!(self.mode, Mode::WrongHandle) {
                return Ok(OwnedObjectPath::try_from("/invalid/handle").unwrap());
            }
            if matches!(self.mode, Mode::Response(_) | Mode::Malformed) {
                if matches!(self.mode, Mode::Malformed) {
                    connection
                        .emit_signal(
                            Some(sender.as_str()),
                            path.clone(),
                            "org.freedesktop.portal.Request",
                            "Response",
                            &("invalid", HashMap::<String, OwnedValue>::new()),
                        )
                        .await
                        .unwrap();
                } else {
                    // Intentionally emit before replying to Print to exercise the race.
                    connection
                        .emit_signal(
                            Some(sender.as_str()),
                            path.clone(),
                            "org.freedesktop.portal.Request",
                            "Response",
                            &(
                                match self.mode {
                                    Mode::Response(status) => status,
                                    _ => 0,
                                },
                                HashMap::<String, OwnedValue>::new(),
                            ),
                        )
                        .await
                        .unwrap();
                }
            }
            Ok(path)
        }
    }

    async fn exercise(mode: Mode, timeout: Duration) -> (Result<Outcome, PrintError>, bool) {
        let bus = Bus::new();
        let server = bus.connect().await;
        server.request_name("org.example.PdfCraftPrintTest").await.unwrap();
        let expected = crate::impose(
            &crate::tests::fixture(3),
            &crate::Settings { pages: vec![2, 0], layout: crate::Layout::multiple(2), ..Default::default() },
        )
        .unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        let cancel = Cancellation::default();
        let handoffs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        server
            .object_server()
            .at(PATH, Mock { mode, cancel: cancel.clone(), expected: expected.clone(), closed: closed.clone(), handoffs: handoffs.clone() })
            .await
            .unwrap();
        let client = bus.connect().await;
        let file = document(&expected).unwrap();
        assert_eq!(file.metadata().unwrap().nlink(), 0, "temporary PDF must already be unlinked");
        let result = print_on(&client, "org.example.PdfCraftPrintTest", &file, "synthetic.pdf", "", &cancel, timeout).await;
        if matches!(mode, Mode::Response(0 | 1)) {
            // Same client and server, a fresh descriptor and request id. A previous cancellation
            // must not poison the next request, nor may a descriptor's offset carry over.
            let next = document(&expected).unwrap();
            let again = print_on(&client, "org.example.PdfCraftPrintTest", &next, "synthetic.pdf", "", &Cancellation::default(), timeout).await;
            assert_eq!(again, result);
        }
        if matches!(mode, Mode::PrepareCancelled | Mode::MissingToken | Mode::InvalidToken | Mode::ConflictingLayout | Mode::UnsupportedOutput) {
            assert_eq!(handoffs.load(Ordering::Relaxed), 0, "no PDF may be submitted after a refused PreparePrint");
        }
        (result, closed.load(Ordering::Relaxed))
    }

    #[test]
    fn dbus_accepts_expected_imposed_pdf_and_consecutive_requests() {
        assert_eq!(future::block_on(exercise(Mode::Response(0), Duration::from_secs(5))).0.unwrap(), Outcome::Accepted);
    }

    #[test]
    fn dbus_cancel_failure_malformed_and_handoff_error() {
        future::block_on(async {
            assert_eq!(exercise(Mode::Response(1), Duration::from_secs(5)).await.0.unwrap(), Outcome::Cancelled);
            assert_eq!(exercise(Mode::PrepareCancelled, Duration::from_secs(5)).await.0.unwrap(), Outcome::Cancelled);
            for mode in [
                Mode::Response(2),
                Mode::Response(99),
                Mode::Malformed,
                Mode::HandoffFailed,
                Mode::WrongHandle,
                Mode::Disconnect,
                Mode::MissingToken,
                Mode::InvalidToken,
                Mode::ConflictingLayout,
                Mode::UnsupportedOutput,
            ] {
                assert!(exercise(mode, Duration::from_secs(5)).await.0.is_err(), "{mode:?}");
            }
        });
    }

    #[test]
    fn pending_request_times_out_and_closes() {
        let (result, closed) = future::block_on(exercise(Mode::Pending, Duration::from_millis(500)));
        assert!(result.unwrap_err().to_string().contains("in time"));
        assert!(closed);
    }

    #[test]
    fn shutdown_cancels_and_closes_pending_dialog() {
        let (result, closed) = future::block_on(exercise(Mode::Shutdown, Duration::from_secs(5)));
        assert_eq!(result.unwrap(), Outcome::Cancelled);
        assert!(closed);
    }

    #[test]
    fn unsupported_or_malformed_version_is_an_error() {
        struct InvalidVersion;
        #[zbus::interface(name = "org.freedesktop.portal.Print")]
        impl InvalidVersion {
            #[zbus(property, name = "version")]
            fn version(&self) -> &str {
                "malformed"
            }
        }
        future::block_on(async {
            let bus = Bus::new();
            let server = bus.connect().await;
            server.request_name("org.example.InvalidPrintPortal").await.unwrap();
            server.object_server().at(PATH, InvalidVersion).await.unwrap();
            let client = bus.connect().await;
            let file = document(b"%PDF-1.7").unwrap();
            assert!(
                print_on(&client, "org.example.InvalidPrintPortal", &file, "synthetic.pdf", "", &Cancellation::default(), Duration::from_secs(5))
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    fn absent_portal_is_actionable() {
        future::block_on(async {
            let bus = Bus::new();
            let connection = bus.connect().await;
            let file = document(b"%PDF-1.7\n").unwrap();
            let result =
                print_on(&connection, "org.example.AbsentPortal", &file, "synthetic.pdf", "", &Cancellation::default(), Duration::from_secs(5)).await;
            let message = result.unwrap_err().to_string();
            assert!(message.contains("xdg-desktop-portal") && message.contains("Save as PDF"));
        });
    }

    #[test]
    fn injected_handoff_error_cannot_become_success_and_leaves_no_named_pdf() {
        struct Failure;
        impl Transport for Failure {
            fn print(&self, file: &File, _: &str, _: &str, _: &Cancellation) -> Result<Outcome, PrintError> {
                assert_eq!(file.metadata().unwrap().nlink(), 0);
                Err(error("document handoff failed"))
            }
        }
        assert!(submit_with(&Failure, b"synthetic", "test", "", &Cancellation::default()).is_err());
        let cancel = Cancellation::default();
        cancel.cancel();
        assert_eq!(submit_with(&Failure, b"synthetic", "test", "", &cancel).unwrap(), Outcome::Cancelled);
    }
}
