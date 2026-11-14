//! The PdfCraft collaboration server.
//!
//! Rooms come from two places: a directory of `<fileId>.pdf` files (the dev
//! mode) or the cabinet control plane, `POST /v1/sessions`, with the file
//! inline (base64) or behind a plain-`http` URL. Each room has a directory under
//! `--data` with a versioned committed base, a journal of the operations
//! accepted since it, and what the cabinet is known to hold: an acknowledged
//! edit survives a crash, a restart replays the tail onto the last base, and a
//! commit is refused (`409`) when the cabinet's copy changed outside the
//! session. Session tokens are HS256 JWTs signed with the cabinet's shared
//! secret, or RS256 JWTs verified with its public key; exactly one scheme is
//! configured at start-up and no token, comment text or PDF byte is logged.
//!
//! ```text
//! pdfcraft-collab-server \
//!   --documents /srv/collab/documents --data /srv/collab/data \
//!   --jwt-secret-file /srv/collab/cabinet-jwt-secret --jwt-audience lzmkm \
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
use pdfcraft_collab_ws::{Config, Hs256Verifier, JwtVerifier, Verifier, WsServer};
use rooms::{CabinetAdapter, Publisher, RoomsConfig};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
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
    jwt_key: Option<PathBuf>,
    jwt_secret_file: Option<PathBuf>,
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
            jwt_key: None,
            jwt_secret_file: None,
            audience: "lzmkm".into(),
            app: "pdfcraft".into(),
            origin: None,
        }
    }
}

/// How session tickets are verified, decided once at start-up (RFC §7).
enum Auth {
    /// RS256 with the cabinet's public key.
    Rsa(PathBuf),
    /// HS256 with the cabinet's shared secret. Never logged.
    Shared(Vec<u8>),
}

impl Auth {
    fn scheme(&self) -> &'static str {
        match self {
            Self::Rsa(_) => "rs256",
            Self::Shared(_) => "hs256",
        }
    }
}

/// Picks exactly one scheme. A key and a secret together, two secrets, or
/// neither are configuration errors: the daemon never guesses which one the
/// operator meant, because guessing is how a ticket that should be refused gets
/// accepted.
fn resolve_auth(key: Option<&Path>, secret_file: Option<&Path>, environment_secret: Option<&str>) -> Result<Auth, String> {
    // An empty variable is what a compose file leaves behind when a secret is
    // not configured; treat it as unset rather than as an empty secret.
    let environment_secret = environment_secret.map(str::trim).filter(|secret| !secret.is_empty());
    match (key, secret_file, environment_secret) {
        (Some(_), Some(_), _) => Err("--jwt-key and --jwt-secret-file are mutually exclusive: use RS256 or HS256, not both".into()),
        (Some(_), None, Some(_)) => Err("--jwt-key and PDFCRAFT_COLLAB_JWT_SECRET are mutually exclusive: use RS256 or HS256, not both".into()),
        (None, Some(_), Some(_)) => Err("--jwt-secret-file and PDFCRAFT_COLLAB_JWT_SECRET both set a secret; keep exactly one".into()),
        (Some(key), None, None) => Ok(Auth::Rsa(key.to_owned())),
        (None, Some(file), None) => Ok(Auth::Shared(read_secret(file)?)),
        (None, None, Some(secret)) => Ok(Auth::Shared(secret.as_bytes().to_vec())),
        (None, None, None) => {
            Err("no session token key: pass --jwt-key FILE (RS256) or --jwt-secret-file FILE / PDFCRAFT_COLLAB_JWT_SECRET (HS256)".into())
        }
    }
}

/// The secret file's text without the trailing whitespace an editor adds. The
/// content never reaches a log or an error message.
fn read_secret(path: &Path) -> Result<Vec<u8>, String> {
    let raw = std::fs::read(path).map_err(|e| format!("could not read the JWT secret {path:?}: {e}"))?;
    let text = String::from_utf8(raw).map_err(|_| format!("the JWT secret {path:?} is not UTF-8 text"))?;
    let secret = text.trim();
    if secret.is_empty() {
        return Err(format!("the JWT secret {path:?} is empty"));
    }
    Ok(secret.as_bytes().to_vec())
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
    let environment_secret = std::env::var("PDFCRAFT_COLLAB_JWT_SECRET").ok();
    let auth = resolve_auth(args.jwt_key.as_deref(), args.jwt_secret_file.as_deref(), environment_secret.as_deref())?;
    let verifier: Arc<dyn Verifier> = match &auth {
        Auth::Rsa(path) => {
            let key = std::fs::read(path).map_err(|e| format!("could not read the JWT key {path:?}: {e}"))?;
            Arc::new(JwtVerifier::from_rsa_pem(&key, &args.audience, &args.app).map_err(|e| format!("the JWT key is not usable: {e}"))?)
        }
        Auth::Shared(secret) => {
            Arc::new(Hs256Verifier::from_secret(secret, &args.audience, &args.app).map_err(|e| format!("the JWT secret is not usable: {e}"))?)
        }
    };

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
    // A room whose committed base the cabinet never took — the POST queued in
    // the process that just died — is resumed from the store and published at
    // once (limitation 6). Without this the room waits for the next commit, join
    // or leave, and a quiet file never reaches the cabinet again.
    let restored = rooms::restore_publications(&mut hub, &rooms).map_err(|e| format!("could not read the publication state: {e:?}"))?;

    let config = Config { app: args.app.clone(), origin: args.origin.clone(), ..Config::default() };
    let server = Arc::new(WsServer::new(hub, verifier.clone(), config));
    let restored = server.restore_publications(restored);
    let handle = server.clone().bind_arc(&args.listen).map_err(|e| format!("could not listen on {}: {e}", args.listen))?;
    // The publication queue (limitation 6): a commit's cabinet POST that failed
    // is retried here with a backoff, without waiting for the next commit or the
    // last client leaving. The handle lives for the life of the process.
    let _publications =
        server.spawn_publication_retries(Duration::from_millis(100)).map_err(|e| format!("could not start the publication queue: {e}"))?;
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
        "pdfcraft-collab-server: {opened} room(s), {restored} restored publication(s), ws://{}, health http://{}{}, auth {}, callback {}",
        handle.local_addr(),
        health,
        control.map_or_else(String::new, |addr| format!(", control http://{addr}/v1/sessions")),
        auth.scheme(),
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
            "--jwt-key" => parsed.jwt_key = Some(value()?.into()),
            "--jwt-secret-file" => parsed.jwt_secret_file = Some(value()?.into()),
            "--jwt-audience" => parsed.audience = value()?,
            "--app" => parsed.app = value()?,
            "--origin" => parsed.origin = Some(value()?),
            "--help" | "-h" => {
                println!(
                    "usage: pdfcraft-collab-server [--listen ADDR] [--health ADDR] [--control ADDR]\n\
                     \x20      [--documents DIR] [--data DIR] [--callback-url URL] [--max-source-bytes N]\n\
                     \x20      [--jwt-key FILE | --jwt-secret-file FILE] [--jwt-audience AUD]\n\
                     \x20      [--app ID] [--origin URL]\n\n\
                     --documents is the dev mode: one room per <fileId>.pdf, no cabinet.\n\
                     --jwt-key verifies RS256 tickets; --jwt-secret-file (or\n\
                     PDFCRAFT_COLLAB_JWT_SECRET) verifies HS256 tickets. Exactly one is\n\
                     required. The commit callback token is read from\n\
                     PDFCRAFT_COLLAB_CALLBACK_TOKEN."
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
            let mut body = format!(
                "pdfcraft_collab_rooms {}\npdfcraft_collab_clients {}\npdfcraft_collab_forced_commits_total {}\npdfcraft_collab_publication_retries_total {}\npdfcraft_collab_publications_restored_total {}\npdfcraft_collab_publications_after_resolve_total {}\npdfcraft_collab_conflicts_resolved_total {}\n",
                metrics.rooms,
                metrics.clients,
                metrics.forced_commits,
                metrics.publication_retries,
                metrics.publications_restored,
                metrics.publications_after_resolve,
                metrics.conflicts_resolved
            );
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
    use super::{Args, Auth, parse_args, render, resolve_auth};
    use crate::rooms::{self, RoomError, RoomsConfig};
    use pdfcraft_collab_hub::Hub;
    use pdfcraft_collab_proto::{ActorId, ClientInfo, ClientMessage, ConflictClass, DocKey, Hello, Op, OpId, OpKind, PageRef, Role};
    use pdfcraft_collab_server::{Adapter, RoomConfig};
    use std::path::Path;

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
    fn the_secret_file_option_parses() {
        let parsed = parse_args(args(&["--jwt-secret-file", "/tmp/cabinet-jwt-secret"]).into_iter()).expect("valid options");
        assert_eq!(parsed.jwt_secret_file, Some("/tmp/cabinet-jwt-secret".into()));
        assert_eq!(parsed.jwt_key, None, "a secret file does not also select RS256");
    }

    #[test]
    fn exactly_one_auth_scheme_is_required() {
        let key = Path::new("/tmp/cabinet-public.pem");
        let secret = Path::new("/tmp/cabinet-jwt-secret");
        let environment = "0123456789abcdef";

        assert!(resolve_auth(None, None, None).is_err(), "neither a key nor a secret must not start");
        assert!(resolve_auth(Some(key), Some(secret), None).is_err(), "a key and a secret file together are ambiguous");
        assert!(resolve_auth(Some(key), None, Some(environment)).is_err(), "a key and an environment secret together are ambiguous");
        assert!(resolve_auth(None, Some(secret), Some(environment)).is_err(), "two secrets are ambiguous");
        assert!(resolve_auth(None, None, Some("   ")).is_err(), "an empty environment secret is not a secret");

        assert!(matches!(resolve_auth(Some(key), None, None), Ok(Auth::Rsa(_))));
        assert!(matches!(resolve_auth(None, None, Some(environment)), Ok(Auth::Shared(_))));
    }

    #[test]
    fn a_secret_file_is_read_without_its_trailing_newline() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("pdfcraft-jwt-secret-{}", std::process::id()));
        std::fs::write(&path, "0123456789abcdef\n")?;
        match resolve_auth(None, Some(path.as_path()), None)? {
            Auth::Shared(secret) => assert_eq!(secret, b"0123456789abcdef"),
            Auth::Rsa(_) => return Err("a secret file must select HS256".into()),
        }
        std::fs::remove_file(&path)?;
        Ok(())
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
    fn a_forced_commit_stops_the_journal_from_growing() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("pdfcraft-collab-forced-{}", std::process::id()));
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
        // The threshold is small so the test does not need thousands of edits;
        // the production default is in `RoomConfig::default`.
        let mut hub = Hub::new(RoomConfig { forced_commit_ops: 3, forced_commit_bytes: 0, ..RoomConfig::default() });
        rooms::seed_rooms(&mut hub, &documents, &config)?;
        let editor = ActorId::new("editor")?;
        let client = hub.connect(editor, Role::Editor, &hello, 0)?;
        let _ = hub.receive(client);

        let journal = config.data.join("file-1").join("journal.jsonl");
        let mut sizes: Vec<u64> = Vec::new();
        let mut forced = 0;
        for i in 0..10u64 {
            let op = Op {
                op_id: OpId::new(format!("forced-{i}"))?,
                doc: doc.clone(),
                base_version: 0,
                epoch: 0,
                kind: OpKind::Primitive,
                class: ConflictClass::Merge,
                target: pdfcraft_collab_proto::Target::Page(PageRef { index: 0, epoch: 0 }),
                payload: serde_json::json!({
                    "action": "annotation.add",
                    "at": [40.0, 250.0],
                    "contents": format!("note {i}"),
                    "nm": format!("forced-{i}"),
                }),
                preview: None,
            };
            hub.send(client, ClientMessage::Op(op), 0)?;
            // What the transport does after every client message: the room
            // decides whether its journal crossed the threshold.
            if let Some(mut pending) = hub.forced_commit(&doc, 0) {
                let outcome = pending.publish();
                let _ = hub.finish(pending, outcome);
                forced += 1;
            }
            sizes.push(std::fs::metadata(&journal).map(|meta| meta.len()).unwrap_or(0));
        }

        let room = hub.room(&doc).ok_or("no room")?;
        assert_eq!(room.version(), 10);
        assert!(forced >= 3, "the room compacted itself while its client stayed connected, not only on leave: {forced}");
        let text = std::fs::read_to_string(&journal)?;
        let left: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
        assert_eq!(left.len(), 1, "the base at version 9 holds the first nine operations");
        // The journal stays at the size of the threshold, not of every accepted
        // operation: three records of the same shape, not ten.
        let line = left[0].len() as u64;
        let largest = sizes.iter().copied().max().unwrap_or(0);
        assert!(largest <= 3 * (line + 8), "the journal never grew past the threshold: {sizes:?}, one line is {line} bytes");
        assert!(sizes.iter().filter(|size| **size == 0).count() >= 3, "every forced commit truncates the journal: {sizes:?}");
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
            let commit = room.commit(&editor, 1)?;
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
        let metrics = pdfcraft_collab_ws::Metrics {
            rooms: 2,
            clients: 3,
            versions: vec![("file-1".into(), 7, 1)],
            poisoned: Vec::new(),
            conflict: Vec::new(),
            forced_commits: 4,
            conflicts_resolved: 1,
            publication_retries: 2,
            publications_restored: 1,
            publications_after_resolve: 3,
        };
        let (status, content_type, body) = render("/health", Ok(metrics.clone()));
        assert_eq!(status, "200 OK");
        assert_eq!(content_type, "application/json");
        assert_eq!(body, r#"{"clients":3,"conflict":[],"ok":true,"poisoned":[],"rooms":2}"#);
        let (status, _, body) = render("/metrics", Ok(metrics));
        assert_eq!(status, "200 OK");
        assert!(body.contains("pdfcraft_collab_rooms 2"));
        assert!(body.contains("pdfcraft_collab_forced_commits_total 4"));
        assert!(body.contains("pdfcraft_collab_publication_retries_total 2"));
        assert!(body.contains("pdfcraft_collab_publications_restored_total 1"));
        assert!(body.contains("pdfcraft_collab_publications_after_resolve_total 3"));
        assert!(body.contains("pdfcraft_collab_conflicts_resolved_total 1"));
        assert!(body.contains(r#"pdfcraft_collab_room_version{room="file-1"} 7"#));
        assert!(!body.contains("read_only"), "a healthy room is not reported as read-only");

        let broken = pdfcraft_collab_ws::Metrics {
            rooms: 1,
            clients: 0,
            versions: Vec::new(),
            poisoned: vec!["file-2".into()],
            conflict: vec!["file-3".into()],
            forced_commits: 0,
            conflicts_resolved: 0,
            publication_retries: 0,
            publications_restored: 0,
            publications_after_resolve: 0,
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
            Ok(pdfcraft_collab_ws::Metrics {
                rooms: 0,
                clients: 0,
                versions: Vec::new(),
                poisoned: Vec::new(),
                conflict: Vec::new(),
                forced_commits: 0,
                conflicts_resolved: 0,
                publication_retries: 0,
                publications_restored: 0,
                publications_after_resolve: 0,
            }),
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
        let seed = rooms::Seed { etag: rooms::etag_of(&bytes), bytes: bytes.clone(), cabinet_version: None, mirror: None };
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
