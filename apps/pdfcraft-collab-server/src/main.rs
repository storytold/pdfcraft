//! The PdfCraft collaboration server.
//!
//! One room per `<fileId>.pdf` in the documents directory. The room's working
//! copy lives in the data directory and is replaced on every commit (manual, or
//! when the last participant leaves and the room is dirty). Session tokens are
//! RS256 JWTs verified with the cabinet's public key; no token, comment text or
//! PDF byte is logged.
//!
//! ```text
//! pdfcraft-collab-server \
//!   --documents /srv/collab/documents --data /srv/collab/data \
//!   --jwt-key /srv/collab/cabinet-public.pem --jwt-audience lzmkm \
//!   --listen 127.0.0.1:8787 --health 127.0.0.1:8788
//! ```
//!
//! `/health` returns JSON, `/metrics` a small Prometheus text. Restart recovery
//! uses the last committed working copy; a crash loses the edits accepted since
//! that commit (the journal is not persisted yet — see `docs/collab/rfc.md`).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_collab_hub::Hub;
use pdfcraft_collab_pdf::PdfAdapter;
use pdfcraft_collab_proto::DocKey;
use pdfcraft_collab_server::RoomConfig;
use pdfcraft_collab_ws::{Config, JwtVerifier, WsServer};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

struct Args {
    listen: String,
    health: String,
    documents: PathBuf,
    data: PathBuf,
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
            documents: "documents".into(),
            data: "collab-data".into(),
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
    let verifier = JwtVerifier::from_rsa_pem(&key, &args.audience, &args.app).map_err(|e| format!("the JWT key is not usable: {e}"))?;

    let mut hub = Hub::new(RoomConfig::default());
    let opened = open_rooms(&mut hub, &args)?;

    let config = Config { app: args.app.clone(), origin: args.origin.clone(), ..Config::default() };
    let server = Arc::new(WsServer::new(hub, Arc::new(verifier), config));
    let handle = server.clone().bind_arc(&args.listen).map_err(|e| format!("could not listen on {}: {e}", args.listen))?;
    let health = start_health(&args.health, server.clone())?;
    println!("pdfcraft-collab-server: {} room(s), ws://{}, health http://{}", opened, handle.local_addr(), health);
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
            "--documents" => parsed.documents = value()?.into(),
            "--data" => parsed.data = value()?.into(),
            "--jwt-key" => parsed.jwt_key = value()?.into(),
            "--jwt-audience" => parsed.audience = value()?,
            "--app" => parsed.app = value()?,
            "--origin" => parsed.origin = Some(value()?),
            "--help" | "-h" => {
                println!(
                    "usage: pdfcraft-collab-server [--listen ADDR] [--health ADDR] [--documents DIR] [--data DIR]\n\
                     \x20      [--jwt-key FILE] [--jwt-audience AUD] [--app ID] [--origin URL]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(parsed)
}

/// Open one room per PDF. A file that already has a working copy is opened from
/// it, so a restart continues from the last commit.
fn open_rooms(hub: &mut Hub<PdfAdapter>, args: &Args) -> Result<usize, String> {
    let entries = std::fs::read_dir(&args.documents).map_err(|e| format!("could not read {:?}: {e}", args.documents))?;
    std::fs::create_dir_all(&args.data).map_err(|e| format!("could not create {:?}: {e}", args.data))?;
    let mut opened = 0;
    for entry in entries {
        let entry = entry.map_err(|e| format!("could not read a document entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pdf") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let doc = DocKey::new(stem).map_err(|e| format!("{stem:?} is not a valid document id: {e}"))?;
        let working = args.data.join(format!("{stem}.pdf"));
        let source = if working.exists() { &working } else { &path };
        let bytes = std::fs::read(source).map_err(|e| format!("could not read {source:?}: {e}"))?;
        if source == &path {
            // The first run starts from the cabinet's copy and keeps it untouched.
            std::fs::write(&working, &bytes).map_err(|e| format!("could not write {working:?}: {e}"))?;
        }
        let mut adapter = PdfAdapter::open(format!("{stem}.pdf"), bytes.into()).map_err(|e| format!("{stem}: {e}"))?;
        adapter.set_commit_path(working);
        hub.open_room(doc, adapter).map_err(|e| format!("{stem}: {e}"))?;
        opened += 1;
    }
    Ok(opened)
}

/// The health and metrics bodies. No document content, token or comment text.
fn render(path: &str, metrics: Result<pdfcraft_collab_ws::Metrics, pdfcraft_collab_ws::TransportError>) -> (&'static str, &'static str, String) {
    match metrics {
        Ok(metrics) if path == "/health" => {
            ("200 OK", "application/json", serde_json::json!({"ok": true, "rooms": metrics.rooms, "clients": metrics.clients}).to_string())
        }
        Ok(metrics) if path == "/metrics" => {
            let mut body = format!("pdfcraft_collab_rooms {}\npdfcraft_collab_clients {}\n", metrics.rooms, metrics.clients);
            for (room, version, epoch) in metrics.versions {
                body.push_str(&format!(
                    "pdfcraft_collab_room_version{{room=\"{room}\"}} {version}\npdfcraft_collab_room_epoch{{room=\"{room}\"}} {epoch}\n"
                ));
            }
            ("200 OK", "text/plain; version=0.0.4", body)
        }
        Ok(_) => ("404 Not Found", "text/plain", "not found\n".to_owned()),
        Err(_) => ("503 Service Unavailable", "text/plain", "unavailable\n".to_owned()),
    }
}

fn start_health(addr: &str, server: Arc<WsServer<PdfAdapter>>) -> Result<std::net::SocketAddr, String> {
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

fn answer(mut stream: TcpStream, server: &WsServer<PdfAdapter>) -> std::io::Result<()> {
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
    use super::{Args, open_rooms, parse_args, render};
    use pdfcraft_collab_hub::Hub;
    use pdfcraft_collab_server::RoomConfig;

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

    #[test]
    fn options_override_the_defaults() {
        let parsed = parse_args(args(&["--listen", "127.0.0.1:9000", "--jwt-audience", "cabinet", "--origin", "https://a.example"]).into_iter())
            .expect("valid options");
        assert_eq!(parsed.listen, "127.0.0.1:9000");
        assert_eq!(parsed.audience, "cabinet");
        assert_eq!(parsed.origin.as_deref(), Some("https://a.example"));
        assert_eq!(parsed.app, "pdfcraft");
    }

    #[test]
    fn a_document_directory_becomes_rooms_with_working_copies() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("pdfcraft-collab-test-{}", std::process::id()));
        let documents = root.join("documents");
        let data = root.join("data");
        std::fs::create_dir_all(&documents)?;
        std::fs::write(documents.join("file-1.pdf"), fixture())?;
        let args = Args { documents: documents.clone(), data: data.clone(), ..Args::default() };
        let mut hub = Hub::new(RoomConfig::default());
        assert_eq!(open_rooms(&mut hub, &args)?, 1);
        assert_eq!(hub.rooms(), 1);
        assert!(data.join("file-1.pdf").exists(), "the working copy is created");
        assert!(documents.join("file-1.pdf").exists(), "the source is untouched");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn a_restart_recovers_the_last_commit() -> Result<(), Box<dyn std::error::Error>> {
        use pdfcraft_collab_proto::{ActorId, ClientInfo, ConflictClass, DocKey, Hello, Op, OpId, OpKind, PageRef, Role, Target};
        let root = std::env::temp_dir().join(format!("pdfcraft-collab-restart-{}", std::process::id()));
        let documents = root.join("documents");
        let data = root.join("data");
        std::fs::create_dir_all(&documents)?;
        std::fs::write(documents.join("file-1.pdf"), fixture())?;
        let args = Args { documents, data, ..Args::default() };
        let doc = DocKey::new("file-1")?;
        let hello = Hello {
            v: 1,
            app: "pdfcraft".into(),
            doc: doc.clone(),
            jwt: "t".into(),
            client: ClientInfo { name: "test".into(), version: "0".into() },
        };
        let editor = ActorId::new("editor")?;
        {
            let mut hub = Hub::new(RoomConfig::default());
            open_rooms(&mut hub, &args)?;
            let room = hub.room_mut(&doc).ok_or("no room")?;
            room.hello(editor.clone(), Role::Editor, &hello, 0)?;
            let op = Op {
                op_id: OpId::new("a1")?,
                doc: doc.clone(),
                base_version: 0,
                epoch: 0,
                kind: OpKind::Primitive,
                class: ConflictClass::Merge,
                target: Target::Page(PageRef { index: 0, epoch: 0 }),
                payload: serde_json::json!({"action": "annotation.add", "at": [40.0, 250.0], "contents": "kept", "nm": "restart-note"}),
                preview: None,
            };
            room.apply(&editor, &op, 0)?;
            room.commit(&editor, 1)?;
        }
        // A new process: the working copy is the base and still has the comment.
        let mut hub = Hub::new(RoomConfig::default());
        open_rooms(&mut hub, &args)?;
        let room = hub.room(&doc).ok_or("no room after restart")?;
        let document = room.adapter().document().ok_or("no document")?;
        assert!(document.info.annotations.iter().any(|a| a.name.as_deref() == Some("restart-note")));
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn health_and_metrics_say_what_they_are() {
        let metrics = pdfcraft_collab_ws::Metrics { rooms: 2, clients: 3, versions: vec![("file-1".into(), 7, 1)] };
        let (status, content_type, body) = render("/health", Ok(metrics.clone()));
        assert_eq!(status, "200 OK");
        assert_eq!(content_type, "application/json");
        assert_eq!(body, r#"{"clients":3,"ok":true,"rooms":2}"#);
        let (status, _, body) = render("/metrics", Ok(metrics));
        assert_eq!(status, "200 OK");
        assert!(body.contains("pdfcraft_collab_rooms 2"));
        assert!(body.contains(r#"pdfcraft_collab_room_version{room="file-1"} 7"#));
        let (status, _, _) = render("/nope", Ok(pdfcraft_collab_ws::Metrics { rooms: 0, clients: 0, versions: Vec::new() }));
        assert_eq!(status, "404 Not Found");
    }

    #[test]
    fn unknown_or_incomplete_options_are_refused() {
        assert!(parse_args(args(&["--nope"]).into_iter()).is_err());
        assert!(parse_args(args(&["--listen"]).into_iter()).is_err());
    }
}
