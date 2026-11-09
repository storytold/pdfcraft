//! Resolving a `conflict` room against the real daemon (RFC §8.2,
//! `docs/collab/STATUS.md` §5 p.6).
//!
//! A commit callback answered `409` flags the room `conflict` and stops the
//! publication queue: retrying the same `expectedVersion` cannot succeed. The
//! operator's way out is `POST /v1/sessions/{fileId}/resolve` with the cabinet's
//! current version and etag. This test runs the whole loop against a real
//! process and a fake cabinet that holds version 7:
//!
//! - a commit is refused with `409`, `/health` and `/metrics` flag the room;
//! - a second commit without a resolution is refused the same way, and the
//!   cabinet sees the same expectation again (the queue is stopped);
//! - the resolve is accepted once, and a second one is refused (`not_conflict`);
//! - `/health` and `/metrics` no longer flag the room and count the resolution;
//! - the next commit reaches the cabinet with `expectedVersion: 7` and one new
//!   version, with its idempotency key still derived from the payload;
//! - the cabinet answers `{version: 8}`, and the commit after that expects 8 —
//!   the resolution survives the publication.

use pdfcraft_collab_proto::{ClientMessage, CommitRequest, ConflictClass, DocKey, Op, OpId, OpKind, PageRef, ServerMessage, Target};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const DOC: &str = "file-1";

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

/// The cabinet's signing key: the daemon verifies RS256 with the public half.
struct Key {
    private_pem: String,
    public_pem: String,
}

fn key() -> Result<Key, Box<dyn std::error::Error>> {
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    let private = RsaPrivateKey::new(&mut rand::thread_rng(), 2048)?;
    Ok(Key { private_pem: private.to_pkcs8_pem(LineEnding::LF)?.to_string(), public_pem: private.to_public_key().to_public_key_pem(LineEnding::LF)? })
}

#[derive(Serialize)]
struct TokenClaims<'a> {
    sub: &'a str,
    aud: &'a str,
    #[serde(rename = "fileId")]
    file_id: &'a str,
    app: &'a str,
    role: &'a str,
    #[serde(rename = "sessionId")]
    session_id: &'a str,
    exp: u64,
}

fn token(key: &Key, subject: &str, role: &str) -> Result<String, Box<dyn std::error::Error>> {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    let exp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + 3_600;
    let claims = TokenClaims { sub: subject, aud: "lzmkm", file_id: DOC, app: "pdfcraft", role, session_id: "session-1", exp };
    let encoding = EncodingKey::from_rsa_pem(key.private_pem.as_bytes())?;
    Ok(encode(&Header::new(Algorithm::RS256), &claims, &encoding)?)
}

/// A free loopback port: bind and drop, so the daemon can take it.
fn free_port() -> Result<u16, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// The cabinet: it holds `version` and answers `409` to any other
/// `expectedVersion`. An accepted commit advances its version by one and the
/// answer names it, the way INTEGRATION.md §5.2 describes.
struct Cabinet {
    url: String,
    posted: Arc<Mutex<Vec<Value>>>,
    held: Arc<AtomicU64>,
}

impl Cabinet {
    fn start(version: u64) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let posted = Arc::new(Mutex::new(Vec::new()));
        let held = Arc::new(AtomicU64::new(version));
        let seen = posted.clone();
        let current = held.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if handle(stream, &seen, &current).is_err() {
                    return;
                }
            }
        });
        Ok(Self { url: format!("http://{address}/commit"), posted, held })
    }

    fn posted(&self) -> Vec<Value> {
        self.posted.lock().map(|posted| posted.clone()).unwrap_or_default()
    }

    fn held(&self) -> u64 {
        self.held.load(Ordering::SeqCst)
    }
}

fn handle(mut stream: TcpStream, posted: &Mutex<Vec<Value>>, held: &AtomicU64) -> Result<(), Box<dyn std::error::Error>> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes)?;
    let body: Value = serde_json::from_slice(&bytes)?;
    let expected = body["expectedVersion"].as_u64().unwrap_or(0);
    let (status, answer) = if expected == held.load(Ordering::SeqCst) {
        let next = held.fetch_add(1, Ordering::SeqCst) + 1;
        (200, json!({"version": next, "etag": format!("sha256:{next:064x}")}))
    } else {
        (409, json!({"code": "conflict", "version": held.load(Ordering::SeqCst)}))
    };
    posted.lock().map_err(|_| "the cabinet lock is poisoned")?.push(body);
    let answer = answer.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
        answer.len()
    );
    let _ = stream.flush();
    Ok(())
}

struct Daemon {
    child: Child,
    addr: SocketAddr,
    health: SocketAddr,
    control: SocketAddr,
    stderr: PathBuf,
}

impl Daemon {
    fn spawn(root: &Path, documents: &Path, data: &Path, public_pem: &Path, callback: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let listen = free_port()?;
        let health = free_port()?;
        let control = free_port()?;
        let stderr = root.join(format!("daemon-{listen}.stderr"));
        let child = Command::new(env!("CARGO_BIN_EXE_pdfcraft-collab-server"))
            .arg("--documents")
            .arg(documents)
            .arg("--data")
            .arg(data)
            .arg("--jwt-key")
            .arg(public_pem)
            .arg("--listen")
            .arg(format!("127.0.0.1:{listen}"))
            .arg("--health")
            .arg(format!("127.0.0.1:{health}"))
            .arg("--control")
            .arg(format!("127.0.0.1:{control}"))
            .arg("--callback-url")
            .arg(callback)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&stderr)?)
            .spawn()?;
        Ok(Self {
            child,
            addr: format!("127.0.0.1:{listen}").parse()?,
            health: format!("127.0.0.1:{health}").parse()?,
            control: format!("127.0.0.1:{control}").parse()?,
            stderr,
        })
    }

    fn died(&self) -> String {
        std::fs::read_to_string(&self.stderr).unwrap_or_default()
    }

    fn join(&self, token: &str) -> Result<Client, Box<dyn std::error::Error>> {
        let url = format!("ws://{}/", self.addr);
        let start = Instant::now();
        let mut delay = Duration::from_millis(20);
        let (mut ws, _) = loop {
            match tungstenite::connect(&url) {
                Ok(connection) => break connection,
                Err(_) if start.elapsed() < Duration::from_secs(10) => {
                    std::thread::sleep(delay);
                    delay = (delay * 2).min(Duration::from_millis(200));
                }
                Err(e) => return Err(format!("could not connect to the daemon at {url}: {e}\n{}", self.died()).into()),
            }
        };
        if let MaybeTlsStream::Plain(stream) = ws.get_mut() {
            stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        }
        ws.send(Message::text(
            json!({
                "type": "hello",
                "v": 1,
                "app": "pdfcraft",
                "doc": DOC,
                "jwt": token,
                "client": {"name": "resolve-test", "version": "0"},
            })
            .to_string(),
        ))?;
        let mut client = Client { ws, version: 0, epoch: 0 };
        match client.next()? {
            ServerMessage::Welcome(welcome) => {
                client.version = welcome.version;
                client.epoch = welcome.epoch;
                Ok(client)
            }
            other => Err(format!("expected a welcome, got {other:?}").into()),
        }
    }

    fn get(&self, path: &str) -> Result<String, Box<dyn std::error::Error>> {
        let mut stream = TcpStream::connect(self.health)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        // Only the request line: the daemon reads one line, and a request head it
        // leaves unread would turn the close into a reset.
        write!(stream, "GET {path} HTTP/1.1\r\n\r\n")?;
        stream.flush()?;
        let mut answer = String::new();
        BufReader::new(stream).read_to_string(&mut answer)?;
        let (_, body) = answer.split_once("\r\n\r\n").ok_or("the health answer has no body")?;
        Ok(body.to_owned())
    }

    /// `POST /v1/sessions/{fileId}/resolve` on the control plane.
    fn resolve(&self, token: Option<&str>, body: &Value) -> Result<(u16, Value), Box<dyn std::error::Error>> {
        let encoded = body.to_string();
        let auth = token.map(|token| format!("Authorization: Bearer {token}\r\n")).unwrap_or_default();
        let mut stream = TcpStream::connect(self.control)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        write!(
            stream,
            "POST /v1/sessions/{DOC}/resolve HTTP/1.1\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{encoded}",
            encoded.len()
        )?;
        stream.flush()?;
        let mut answer = String::new();
        BufReader::new(stream).read_to_string(&mut answer)?;
        let (head, body) = answer.split_once("\r\n\r\n").ok_or("the resolve answer has no body")?;
        let status: u16 = head.split_whitespace().nth(1).and_then(|code| code.parse().ok()).ok_or("the resolve answer has no status")?;
        Ok((status, serde_json::from_str(body)?))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Client {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    version: u64,
    epoch: u64,
}

impl Client {
    fn next(&mut self) -> Result<ServerMessage, Box<dyn std::error::Error>> {
        loop {
            match self.ws.read()? {
                Message::Text(text) => return Ok(serde_json::from_str(text.as_str())?),
                Message::Close(frame) => return Err(format!("the server closed the connection: {frame:?}").into()),
                _ => {}
            }
        }
    }

    fn apply(&mut self, op: Op) -> Result<(), Box<dyn std::error::Error>> {
        self.ws.send(Message::text(serde_json::to_string(&ClientMessage::Op(op))?))?;
        loop {
            match self.next()? {
                ServerMessage::Applied(applied) => {
                    self.version = applied.version;
                    self.epoch = applied.epoch;
                    return Ok(());
                }
                ServerMessage::Rejected(rejected) => return Err(format!("the operation was refused: {rejected:?}").into()),
                ServerMessage::Presence(_) => {}
                other => return Err(format!("expected an applied, got {other:?}").into()),
            }
        }
    }

    /// Send the commit and read the first answer: a result or the failed
    /// publication the daemon reports to the committer.
    fn commit(&mut self) -> Result<ServerMessage, Box<dyn std::error::Error>> {
        self.ws
            .send(Message::text(serde_json::to_string(&ClientMessage::Commit(CommitRequest { expected_version: self.version, message: None }))?))?;
        loop {
            match self.next()? {
                ServerMessage::Presence(_) => {}
                other => return Ok(other),
            }
        }
    }
}

fn note(nm: &str, contents: &str) -> Value {
    json!({"action": "annotation.add", "at": [40.0, 250.0], "contents": contents, "nm": nm})
}

fn op(id: &str, payload: Value, base_version: u64, epoch: u64) -> Result<Op, Box<dyn std::error::Error>> {
    Ok(Op {
        op_id: OpId::new(id)?,
        doc: DocKey::new(DOC)?,
        base_version,
        epoch,
        kind: OpKind::Primitive,
        class: ConflictClass::Merge,
        target: Target::Page(PageRef { index: 0, epoch }),
        payload,
        preview: None,
    })
}

/// The daemon in dev mode, a fake cabinet holding version 7, and the signing
/// key: every test below starts here.
fn environment() -> Result<(PathBuf, Daemon, Cabinet, String), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("pdfcraft-collab-resolve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let documents = root.join("documents");
    let data = root.join("data");
    std::fs::create_dir_all(&documents)?;
    std::fs::create_dir_all(&data)?;
    std::fs::write(documents.join(format!("{DOC}.pdf")), fixture())?;
    let key = key()?;
    let public_pem = root.join("cabinet-public.pem");
    std::fs::write(&public_pem, key.public_pem.as_bytes())?;
    let token = token(&key, "ada", "editor")?;
    let cabinet = Cabinet::start(7)?;
    let daemon = Daemon::spawn(&root, &documents, &data, &public_pem, &cabinet.url)?;
    Ok((root, daemon, cabinet, token))
}

/// The etag of the source file: in dev mode that file is the cabinet's copy.
fn source_etag() -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(fixture());
    format!("sha256:{}", digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>())
}

fn commit_refused(client: &mut Client) -> Result<(), Box<dyn std::error::Error>> {
    match client.commit()? {
        ServerMessage::Error(error) => {
            assert!(!error.retryable, "a 409 is not retryable: {error:?}");
            Ok(())
        }
        other => Err(format!("expected the refused publication, got {other:?}").into()),
    }
}

fn commit_accepted(client: &mut Client, version: u64) -> Result<pdfcraft_collab_proto::CommitResult, Box<dyn std::error::Error>> {
    match client.commit()? {
        ServerMessage::CommitResult(result) => {
            assert_eq!(result.version, version, "the commit is the room's next version");
            Ok(result)
        }
        other => Err(format!("expected a commit result, got {other:?}").into()),
    }
}

/// `/health` and `/metrics` flag the room, and no resolution is counted yet.
fn assert_flagged(daemon: &Daemon) -> Result<(), Box<dyn std::error::Error>> {
    let health = daemon.get("/health")?;
    assert!(health.contains(r#""conflict":["file-1"]"#), "the room is flagged: {health}");
    let metrics = daemon.get("/metrics")?;
    assert!(metrics.contains(r#"pdfcraft_collab_room_conflict{room="file-1"} 1"#), "{metrics}");
    assert!(metrics.contains("pdfcraft_collab_conflicts_resolved_total 0"), "{metrics}");
    Ok(())
}

/// `/health` and `/metrics` say the conflict is gone and count one resolution.
fn assert_resolved(daemon: &Daemon) -> Result<(), Box<dyn std::error::Error>> {
    let health = daemon.get("/health")?;
    assert!(health.contains(r#""conflict":[]"#), "the flag is gone: {health}");
    let metrics = daemon.get("/metrics")?;
    assert!(!metrics.contains("pdfcraft_collab_room_conflict"), "no room is flagged any more: {metrics}");
    assert!(metrics.contains("pdfcraft_collab_conflicts_resolved_total 1"), "{metrics}");
    Ok(())
}

/// A delivery of `version` that names `expected` and carries the derived key.
fn assert_delivery(posted: &Value, version: u64, expected: u64, sha256: &str) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(posted["version"], version);
    assert_eq!(posted["expectedVersion"], expected, "the cabinet's version is what it expects next");
    let key = format!("file-1:{version}:{sha256}");
    assert_eq!(posted["idempotencyKey"], key, "the key stays derived from the payload, not from the attempt");
    Ok(())
}

#[test]
fn a_conflict_is_resolved_once_and_the_next_commits_publish_again() -> Result<(), Box<dyn std::error::Error>> {
    let (root, daemon, cabinet, token) = environment()?;
    let mut client = daemon.join(&token)?;
    assert_eq!((client.version, client.epoch), (0, 0));

    client.apply(op("resolve-1", note("resolve-1", "kept"), 0, 0)?)?;
    commit_refused(&mut client)?;
    assert_flagged(&daemon)?;

    // Without a resolution the queue stays stopped: another commit is refused
    // with the same expectation and the room is still flagged.
    commit_refused(&mut client)?;
    let posted = cabinet.posted();
    assert_eq!(posted.len(), 2, "two refused deliveries: {posted:?}");
    assert_eq!(posted[0]["expectedVersion"], 0);
    assert_eq!(posted[1]["expectedVersion"], 0, "an unresolved room repeats the refused version, it does not guess");

    let etag = source_etag();
    let resolve = |version: u64, etag: &str| json!({"expectedVersion": version, "etag": etag});

    let (status, reply) = daemon.resolve(None, &resolve(7, &etag))?;
    assert_eq!(status, 401, "a resolve needs a service token: {reply}");
    let (status, reply) = daemon.resolve(Some(&token), &resolve(7, &etag))?;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["fileId"], DOC);
    assert_eq!(reply["version"], 1, "the resolve does not move the document");
    assert_eq!(reply["cabinetVersion"], 7);
    assert_eq!(reply["conflict"], false);
    assert_resolved(&daemon)?;

    // A second resolve has nothing to do.
    let (status, reply) = daemon.resolve(Some(&token), &resolve(8, &format!("sha256:{:064x}", 8)))?;
    assert_eq!(status, 409, "{reply}");
    assert_eq!(reply["code"], "not_conflict");

    // The next commit carries the resolved version and reaches the cabinet
    // once. The cabinet answers `{version: 8}`, so the commit after that
    // expects 8: the resolution holds for the rest of the session.
    client.apply(op("resolve-2", note("resolve-2", "after"), 1, 0)?)?;
    let result = commit_accepted(&mut client, 2)?;
    let posted = cabinet.posted();
    let accepted = posted.last().ok_or("the resolved commit was not delivered")?;
    assert_delivery(accepted, 2, 7, &result.sha256)?;
    assert_eq!(cabinet.held(), 8, "the cabinet stored one version for the commit");

    client.apply(op("resolve-3", note("resolve-3", "later"), 2, 0)?)?;
    let third = commit_accepted(&mut client, 3)?;
    let posted = cabinet.posted();
    let delivered = posted.last().ok_or("the third commit was not delivered")?;
    assert_delivery(delivered, 3, 8, &third.sha256)?;
    assert_eq!(cabinet.held(), 9);
    let metrics = daemon.get("/metrics")?;
    assert!(metrics.contains("pdfcraft_collab_conflicts_resolved_total 1"), "the refused second resolve was not counted: {metrics}");

    drop(daemon);
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
