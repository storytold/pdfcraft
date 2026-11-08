//! The PdfCraft collaboration server.
//!
//! Rooms come from two places: a directory of `<fileId>.pdf` files (the dev
//! mode) or the cabinet control plane, `POST /v1/sessions`, with the file
//! inline (base64) or behind a plain-`http` URL. Each room has a directory under
//! `--data` with a versioned committed base, a journal of the operations
//! accepted since it, and what the cabinet is known to hold: an acknowledged
//! edit survives a crash, a restart replays the tail onto the last base, and a
//! commit is refused (`409`) when the cabinet's copy changed outside the
//! session. Session tokens are RS256 JWTs verified with the cabinet's public
//! key; no token, comment text or PDF byte is logged.
//!
//! ```text
//! pdfcraft-collab-server \
//!   --documents /srv/collab/documents --data /srv/collab/data \
//!   --jwt-key /srv/collab/cabinet-public.pem --jwt-audience lzmkm \
//!   --listen 127.0.0.1:8787 --health 127.0.0.1:8788 \
//!   --control 127.0.0.1:8789 --callback-url http://cabinet.internal/v1/collab/commit
//! ```
//!
//! `/health` returns JSON, `/metrics` a small Prometheus text. A room whose
//! journal failed becomes read-only and is listed under `poisoned`; a room whose
//! commit was refused because the cabinet moved on is listed under `conflict`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod control;
mod http;
mod rooms;

use control::{Control, ControlConfig};
use pdfcraft_collab_hub::Hub;
use pdfcraft_collab_server::RoomConfig;
use pdfcraft_collab_ws::{Config, JwtVerifier, Verifier, WsServer};
use rooms::{CabinetAdapter, Publisher, RoomsConfig};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

/// The largest file the control plane accepts. The base64 body of a file this
/// size is about 1.37 times bigger.
const DEFAULT_MAX_SOURCE: usize = 256 << 20;

struct Args {
    listen: String,
    health: String,
    control: Option<String>,
    documents: Option<PathBuf>,
    data: PathBuf,
    callback: Option<String>,
    max_source_bytes: usize,
    jwt_key: PathBuf,
    audience: String,
    app: String,
    origin: Option<String>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8787".into(),
            health: "127.0.0.1:8788".into(),
            control: None,
            documents: None,
            data: "collab-data".into(),
            callback: None,
            max_source_bytes: DEFAULT_MAX_SOURCE,
            jwt_key: "cabinet-public.pem".into(),
            audience: "lzmkm".into(),
            app: "pdfcraft".into(),
            origin: None,
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pdfcraft-collab-server: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1))?;
    let key = std::fs::read(&args.jwt_key).map_err(|e| format!("could not read the JWT key {:?}: {e}", args.jwt_key))?;
    let verifier: Arc<dyn Verifier> =
        Arc::new(JwtVerifier::from_rsa_pem(&key, &args.audience, &args.app).map_err(|e| format!("the JWT key is not usable: {e}"))?);

    let publisher = args
        .callback
        .as_ref()
        .map(|url| Arc::new(Publisher::new(url.clone(), std::env::var("PDFCRAFT_COLLAB_CALLBACK_TOKEN").ok(), Duration::from_secs(10))));
    let rooms = RoomsConfig {
        data: args.data.clone(),
        publisher: publisher.clone(),
        max_source_bytes: args.max_source_bytes,
        http_timeout: Duration::from_secs(10),
    };

    let mut hub = Hub::new(RoomConfig::default());
    let opened = match &args.documents {
        Some(documents) => rooms::seed_rooms(&mut hub, documents, &rooms).map_err(|e| format!("could not open the seeded rooms: {e:?}"))?,
        // Production: rooms arrive through the control plane.
        None => 0,
    };

    let config = Config { app: args.app.clone(), origin: args.origin.clone(), ..Config::default() };
    let server = Arc::new(WsServer::new(hub, verifier.clone(), config));
    let handle = server.clone().bind_arc(&args.listen).map_err(|e| format!("could not listen on {}: {e}", args.listen))?;
    let health = start_health(&args.health, server.clone())?;
    let control = match &args.control {
        Some(addr) => {
            let control = Arc::new(Control::new(server.clone(), verifier, ControlConfig { rooms }));
            let bound = control.bind(addr)?;
            Some(bound)
        }
        None => None,
    };
    println!(
        "pdfcraft-collab-server: {opened} room(s), ws://{}, health http://{}{}, callback {}",
        handle.local_addr(),
        health,
        control.map_or_else(String::new, |addr| format!(", control http://{addr}/v1/sessions")),
        publisher.as_ref().map_or("none", |publisher| publisher.url()),
    );
    loop {
        std::thread::park();
    }
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args::default();
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--listen" => parsed.listen = value()?,
            "--health" => parsed.health = value()?,
            "--control" => parsed.control = Some(value()?),
            "--documents" => parsed.documents = Some(value()?.into()),
            "--data" => parsed.data = value()?.into(),
            "--callback-url" => parsed.callback = Some(value()?),
            "--max-source-bytes" => parsed.max_source_bytes = value()?.parse().map_err(|_| "--max-source-bytes needs a number".to_owned())?,
            "--jwt-key" => parsed.jwt_key = value()?.into(),
            "--jwt-audience" => parsed.audience = value()?,
            "--app" => parsed.app = value()?,
            "--origin" => parsed.origin = Some(value()?),
            "--help" | "-h" => {
                println!(
                    "usage: pdfcraft-collab-server [--listen ADDR] [--health ADDR] [--control ADDR]\n\
                     \x20      [--documents DIR] [--data DIR] [--callback-url URL] [--max-source-bytes N]\n\
                     \x20      [--jwt-key FILE] [--jwt-audience AUD] [--app ID] [--origin URL]\n\n\
                     --documents is the dev mode: one room per <fileId>.pdf, no cabinet.\n\
                     The commit callback token is read from PDFCRAFT_COLLAB_CALLBACK_TOKEN."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    if parsed.max_source_bytes == 0 {
        return Err("--max-source-bytes must be positive".into());
    }
    Ok(parsed)
}

/// The health and metrics bodies. No document content, token or comment text.
fn render(path: &str, metrics: Result<pdfcraft_collab_ws::Metrics, pdfcraft_collab_ws::TransportError>) -> (&'static str, &'static str, String) {
    match metrics {
        Ok(metrics) if path == "/health" => (
            "200 OK",
            "application/json",
            serde_json::json!({
                "ok": metrics.poisoned.is_empty(),
                "rooms": metrics.rooms,
                "clients": metrics.clients,
                "poisoned": metrics.poisoned,
                "conflict": metrics.conflict,
            })
            .to_string(),
        ),
        Ok(metrics) if path == "/metrics" => {
            let mut body = format!("pdfcraft_collab_rooms {}\npdfcraft_collab_clients {}\n", metrics.rooms, metrics.clients);
            for (room, version, epoch) in metrics.versions {
                body.push_str(&format!(
                    "pdfcraft_collab_room_version{{room=\"{room}\"}} {version}\npdfcraft_collab_room_epoch{{room=\"{room}\"}} {epoch}\n"
                ));
            }
            for room in metrics.poisoned {
                body.push_str(&format!("pdfcraft_collab_room_read_only{{room=\"{room}\"}} 1\n"));
            }
            for room in metrics.conflict {
                body.push_str(&format!("pdfcraft_collab_room_conflict{{room=\"{room}\"}} 1\n"));
            }
            ("200 OK", "text/plain; version=0.0.4", body)
        }
        Ok(_) => ("404 Not Found", "text/plain", "not found\n".to_owned()),
        Err(_) => ("503 Service Unavailable", "text/plain", "unavailable\n".to_owned()),
    }
}

fn start_health(addr: &str, server: Arc<WsServer<CabinetAdapter>>) -> Result<std::net::SocketAddr, String> {
    let listener = TcpListener::bind(addr).map_err(|e| format!("could not listen for health on {addr}: {e}"))?;
    let bound = listener.local_addr().map_err(|e| format!("health listener: {e}"))?;
    std::thread::Builder::new()
        .name("pdfcraft-collab-health".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = answer(stream, &server);
            }
        })
        .map_err(|e| format!("could not start the health listener: {e}"))?;
    Ok(bound)
}

fn answer(mut stream: TcpStream, server: &WsServer<CabinetAdapter>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (status, content_type, body) = render(path, server.metrics());
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

#[cfg(test)]
mod tests {
    use super::{Args, parse_args, render};
    use crate::rooms::{self, RoomError, RoomsConfig};
    use pdfcraft_collab_hub::Hub;
    use pdfcraft_collab_proto::{ActorId, ClientInfo, ConflictClass, DocKey, Hello, Op, OpId, OpKind, PageRef, Role};
    use pdfcraft_collab_server::{Adapter, RoomConfig};

    fn fixture() -> Vec<u8> {
        let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
        objs.push(b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec());
        objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
        objs.push(b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec());
        let body = "BT /F1 24 Tf 20 150 Td (Page 1) Tj ET";
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn rooms_config(root: &std::path::Path) -> RoomsConfig {
        RoomsConfig { data: root.join("data"), publisher: None, max_source_bytes: 8 << 20, http_timeout: std::time::Duration::from_secs(5) }
    }

    #[test]
    fn options_override_the_defaults() {
        let parsed = parse_args(args(&["--listen", "127.0.0.1:9000", "--jwt-audience", "cabinet", "--origin", "https://a.example"]).into_iter())
            .expect("valid options");
        assert_eq!(parsed.listen, "127.0.0.1:9000");
        assert_eq!(parsed.audience, "cabinet");
        assert_eq!(parsed.origin.as_deref(), Some("https://a.example"));
        assert_eq!(parsed.app, "pdfcraft");
        assert_eq!(parsed.documents, None, "no directory is the production default");
    }

    #[test]
    fn the_control_plane_callback_and_limits_parse() {
        let parsed =
            parse_args(args(&["--control", "127.0.0.1:8789", "--callback-url", "http://cabinet/commit", "--max-source-bytes", "1024"]).into_iter())
                .expect("valid options");
        assert_eq!(parsed.control.as_deref(), Some("127.0.0.1:8789"));
        assert_eq!(parsed.callback.as_deref(), Some("http://cabinet/commit"));
        assert_eq!(parsed.max_source_bytes, 1024);
    }

    #[test]
    fn a_document_directory_becomes_rooms_with_a_committed_base() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("pdfcraft-collab-test-{}", std::process::id()));
        let documents = root.join("documents");
        let data = root.join("data");
        std::fs::create_dir_all(&documents)?;
        std::fs::write(documents.join("file-1.pdf"), fixture())?;
        let config = rooms_config(&root);
        let mut hub = Hub::new(RoomConfig::default());
        assert_eq!(rooms::seed_rooms(&mut hub, &documents, &config)?, 1);
        assert_eq!(hub.rooms(), 1);
        assert!(data.join("file-1/versions/0/file.pdf").exists(), "version 0 is the cabinet's copy");
        assert!(data.join("file-1/CURRENT").exists());
        assert!(data.join("file-1/CABINET").exists(), "the etag the room is compared against is recorded");
        assert!(documents.join("file-1.pdf").exists(), "the source is untouched");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn a_restart_recovers_the_last_commit() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("pdfcraft-collab-restart-{}", std::process::id()));
        let documents = root.join("documents");
        std::fs::create_dir_all(&documents)?;
        std::fs::write(documents.join("file-1.pdf"), fixture())?;
        let config = rooms_config(&root);
        let doc = DocKey::new("file-1")?;
        let hello = Hello {
            v: 1,
            app: "pdfcraft".into(),
            doc: doc.clone(),
            jwt: "t".into(),
            client: ClientInfo { name: "test".into(), version: "0".into() },
        };
        let editor = ActorId::new("editor")?;
        let add = |id: &str, nm: &str, base: u64| -> Result<Op, Box<dyn std::error::Error>> {
            Ok(Op {
                op_id: OpId::new(id)?,
                doc: DocKey::new("file-1")?,
                base_version: base,
                epoch: 0,
                kind: OpKind::Primitive,
                class: ConflictClass::Merge,
                target: pdfcraft_collab_proto::Target::Page(PageRef { index: 0, epoch: 0 }),
                payload: serde_json::json!({"action": "annotation.add", "at": [40.0, 250.0], "contents": "kept", "nm": nm}),
                preview: None,
            })
        };
        {
            let mut hub = Hub::new(RoomConfig::default());
            rooms::seed_rooms(&mut hub, &documents, &config)?;
            let room = hub.room_mut(&doc).ok_or("no room")?;
            room.hello(editor.clone(), Role::Editor, &hello, 0)?;
            room.apply(&editor, &add("a1", "committed-note", 0)?, 0)?;
            let mut commit = room.commit(&editor, 1)?;
            let outcome = commit.publish();
            room.finish(commit, outcome)?;
            // Accepted after the commit: the journal, not the base, holds it.
            room.apply(&editor, &add("a2", "journal-note", 1)?, 0)?;
        }
        // A new process: the base plus the replayed journal, in that order.
        let mut hub = Hub::new(RoomConfig::default());
        rooms::seed_rooms(&mut hub, &documents, &config)?;
        let room = hub.room(&doc).ok_or("no room after restart")?;
        let document = room.adapter().pdf().document().ok_or("no document")?;
        assert!(document.info.annotations.iter().any(|a| a.name.as_deref() == Some("committed-note")));
        assert!(
            document.info.annotations.iter().any(|a| a.name.as_deref() == Some("journal-note")),
            "an operation acknowledged after the last commit survives a crash"
        );
        assert_eq!(room.version(), 2, "the head continues past the committed version");
        assert!(room.is_dirty(), "the replayed operation is not in the base yet, so the room must commit again");
        // `Adapter::published` still names the committed version: a restart
        // knows the cabinet does not have the replayed operation.
        let published = Adapter::published(room.adapter()).ok_or("no cabinet marker")?;
        assert_eq!(published.version, 1);
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn health_and_metrics_say_what_they_are() {
        let metrics =
            pdfcraft_collab_ws::Metrics { rooms: 2, clients: 3, versions: vec![("file-1".into(), 7, 1)], poisoned: Vec::new(), conflict: Vec::new() };
        let (status, content_type, body) = render("/health", Ok(metrics.clone()));
        assert_eq!(status, "200 OK");
        assert_eq!(content_type, "application/json");
        assert_eq!(body, r#"{"clients":3,"conflict":[],"ok":true,"poisoned":[],"rooms":2}"#);
        let (status, _, body) = render("/metrics", Ok(metrics));
        assert_eq!(status, "200 OK");
        assert!(body.contains("pdfcraft_collab_rooms 2"));
        assert!(body.contains(r#"pdfcraft_collab_room_version{room="file-1"} 7"#));
        assert!(!body.contains("read_only"), "a healthy room is not reported as read-only");

        let broken = pdfcraft_collab_ws::Metrics {
            rooms: 1,
            clients: 0,
            versions: Vec::new(),
            poisoned: vec!["file-2".into()],
            conflict: vec!["file-3".into()],
        };
        let (_, _, body) = render("/health", Ok(broken.clone()));
        assert!(body.contains(r#""ok":false"#));
        assert!(body.contains(r#""poisoned":["file-2"]"#));
        assert!(body.contains(r#""conflict":["file-3"]"#));
        let (_, _, body) = render("/metrics", Ok(broken));
        assert!(body.contains(r#"pdfcraft_collab_room_read_only{room="file-2"} 1"#));
        assert!(body.contains(r#"pdfcraft_collab_room_conflict{room="file-3"} 1"#));

        let (status, _, _) = render(
            "/nope",
            Ok(pdfcraft_collab_ws::Metrics { rooms: 0, clients: 0, versions: Vec::new(), poisoned: Vec::new(), conflict: Vec::new() }),
        );
        assert_eq!(status, "404 Not Found");
    }

    #[test]
    fn unknown_or_incomplete_options_are_refused() {
        assert!(parse_args(args(&["--nope"]).into_iter()).is_err());
        assert!(parse_args(args(&["--listen"]).into_iter()).is_err());
        assert!(parse_args(args(&["--max-source-bytes", "0"]).into_iter()).is_err());
    }

    #[test]
    fn a_missing_seed_directory_is_an_error() {
        let root = std::env::temp_dir().join(format!("pdfcraft-collab-missing-{}", std::process::id()));
        let mut hub = Hub::new(RoomConfig::default());
        assert!(matches!(rooms::seed_rooms(&mut hub, &root.join("nope"), &rooms_config(&root)), Err(RoomError::Invalid(_))));
    }

    #[test]
    fn a_created_room_reports_what_the_cabinet_holds() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("pdfcraft-collab-open-{}", std::process::id()));
        let config = rooms_config(&root);
        let doc = DocKey::new("file-1")?;
        let bytes = std::sync::Arc::new(fixture());
        let seed = rooms::Seed { etag: rooms::etag_of(&bytes), bytes: bytes.clone(), mirror: None };
        let mut hub = Hub::new(RoomConfig::default());
        let opened = rooms::open_room(&mut hub, &config, doc, Some(seed))?;
        assert!(opened.created);
        assert_eq!(opened.version, 0);
        assert_eq!(opened.etag, Some(rooms::etag_of(&bytes)));
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn args_keep_the_optional_documents_directory() {
        let parsed = parse_args(args(&["--documents", "/tmp/docs"]).into_iter()).expect("valid options");
        assert_eq!(parsed.documents, Some("/tmp/docs".into()));
    }

    #[test]
    fn the_default_args_are_off() {
        let parsed = Args::default();
        assert!(parsed.control.is_none());
        assert!(parsed.documents.is_none());
        assert!(parsed.callback.is_none());
    }
}
