//! The publication queue against a restart and an operator's resolve
//! (RFC §8.2, `docs/collab/STATUS.md` §5, limitation 6).
//!
//! A commit writes its base locally and then POSTs the file to the cabinet. The
//! queue that retries a failed POST lives in the process, so before this test
//! existed a room whose publication never landed waited for the next commit, a
//! join or the last leave — a file nobody touched stayed unpublished forever,
//! and an operator's `POST /v1/sessions/{fileId}/resolve` only cleared the flag
//! for a commit that might never come.
//!
//! These tests run the real daemon with the control plane (so rooms are opened
//! from the store, not from a `--documents` directory) and kill it with
//! `SIGKILL` right after a failed publication:
//!
//! - the restart publishes the base the previous process wrote, with the same
//!   derived idempotency key, without any client connecting;
//! - a cabinet that refuses that publication (`409`) stops the queue and flags
//!   the room, and the resolve publishes the base at once — again with no
//!   commit, join or leave.

use pdfcraft_collab_proto::{ClientMessage, CommitRequest, ConflictClass, DocKey, Op, OpId, OpKind, PageRef, ServerMessage, Target};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// Standard base64, so the session request can carry the file inline. The
/// daemon decodes it with its own encoder's counterpart.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(n >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 0x3f] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 0x3f] as char } else { '=' });
    }
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

fn free_port() -> Result<u16, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// One commit callback the cabinet saw.
#[derive(Debug, Clone)]
struct Posted {
    idempotency_key: Option<String>,
    body: Value,
}

/// The cabinet's commit endpoint. It holds one version: a publication whose
/// `expectedVersion` is that one advances it by one and answers
/// `{"version","etag"}`; any other is refused with `409`. While `down` is set
/// every call answers `503`, the way a cabinet in maintenance does.
struct Cabinet {
    url: String,
    posted: Arc<Mutex<Vec<Posted>>>,
    held: Arc<AtomicU64>,
}

impl Cabinet {
    fn spawn(held: u64, down: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let posted = Arc::new(Mutex::new(Vec::new()));
        let held = Arc::new(AtomicU64::new(held));
        let down = Arc::new(AtomicBool::new(down));
        let seen = posted.clone();
        let current = held.clone();
        let unavailable = down.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if handle(stream, &seen, &current, &unavailable).is_err() {
                    return;
                }
            }
        });
        Ok(Self { url: format!("http://{address}/commit"), posted, held })
    }

    fn posted(&self) -> Vec<Posted> {
        self.posted.lock().map(|posted| posted.clone()).unwrap_or_default()
    }

    fn held(&self) -> u64 {
        self.held.load(Ordering::SeqCst)
    }

    /// Wait until the cabinet saw at least `count` calls. The daemon publishes
    /// from its own thread, so every assertion about a publication waits.
    fn wait_for(&self, count: usize, limit: Duration) -> Result<Vec<Posted>, String> {
        let deadline = Instant::now() + limit;
        loop {
            let posted = self.posted();
            if posted.len() >= count {
                return Ok(posted);
            }
            if Instant::now() > deadline {
                return Err(format!("the cabinet saw {} call(s), not {count}: {posted:?}", posted.len()));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

fn handle(mut stream: TcpStream, posted: &Mutex<Vec<Posted>>, held: &AtomicU64, down: &AtomicBool) -> Result<(), Box<dyn std::error::Error>> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut length = 0usize;
    let mut idempotency_key = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        if let Some(value) = lower.strip_prefix("idempotency-key:") {
            idempotency_key = Some(value.trim().to_owned());
        }
    }
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes)?;
    let body: Value = serde_json::from_slice(&bytes)?;
    posted.lock().map_err(|_| "the cabinet lock is poisoned")?.push(Posted { idempotency_key, body: body.clone() });
    if down.load(Ordering::SeqCst) {
        let answer = "{}";
        let _ = write!(
            stream,
            "HTTP/1.1 503 X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
            answer.len()
        );
        let _ = stream.flush();
        return Ok(());
    }
    let expected = body["expectedVersion"].as_u64().unwrap_or(0);
    let (status, answer) = if expected == held.load(Ordering::SeqCst) {
        let next = held.fetch_add(1, Ordering::SeqCst) + 1;
        (200, json!({"version": next, "etag": format!("sha256:{next:064x}")}))
    } else {
        (409, json!({"code": "conflict", "version": held.load(Ordering::SeqCst)}))
    };
    let answer = answer.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
        answer.len()
    );
    let _ = stream.flush();
    Ok(())
}

/// The daemon without `--documents`: rooms come from the control plane, and a
/// restart finds them in the store (the production path).
struct Daemon {
    child: Child,
    addr: SocketAddr,
    health: SocketAddr,
    control: SocketAddr,
    stderr: PathBuf,
}

impl Daemon {
    fn spawn(root: &Path, data: &Path, public_pem: &Path, callback: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let listen = free_port()?;
        let health = free_port()?;
        let control = free_port()?;
        let stderr = root.join(format!("daemon-{listen}.stderr"));
        let child = Command::new(env!("CARGO_BIN_EXE_pdfcraft-collab-server"))
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
                "client": {"name": "restore-test", "version": "0"},
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
        write!(stream, "GET {path} HTTP/1.1\r\n\r\n")?;
        stream.flush()?;
        let mut answer = String::new();
        BufReader::new(stream).read_to_string(&mut answer)?;
        let (_, body) = answer.split_once("\r\n\r\n").ok_or("the health answer has no body")?;
        Ok(body.to_owned())
    }

    /// `GET /health` until the body contains `needle`: the restored publication
    /// is reported by the room after the callback answered.
    fn wait_for_health(&self, needle: &str, limit: Duration) -> Result<String, String> {
        let deadline = Instant::now() + limit;
        loop {
            let body = self.get("/health").map_err(|e| e.to_string())?;
            if body.contains(needle) {
                return Ok(body);
            }
            if Instant::now() > deadline {
                return Err(format!("the health answer never contained {needle:?}: {body}"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// `POST /v1/sessions` with the file inline.
    fn session(&self, token: &str, bytes: &[u8]) -> Result<(u16, Value), Box<dyn std::error::Error>> {
        let body = json!({"fileId": DOC, "jwt": token, "bytes": base64(bytes)});
        self.post("/v1/sessions", Some(token), &body)
    }

    /// `POST /v1/sessions/{fileId}/resolve` on the control plane.
    fn resolve(&self, token: Option<&str>, body: &Value) -> Result<(u16, Value), Box<dyn std::error::Error>> {
        self.post(&format!("/v1/sessions/{DOC}/resolve"), token, body)
    }

    fn post(&self, path: &str, token: Option<&str>, body: &Value) -> Result<(u16, Value), Box<dyn std::error::Error>> {
        // The control plane is bound after the store was read, so the first
        // request may arrive before the process is listening.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match TcpStream::connect(self.control) {
                Ok(stream) => break stream,
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
                Err(e) => return Err(format!("could not connect to the control plane: {e}\n{}", self.died()).into()),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        let auth = token.map(|token| format!("Authorization: Bearer {token}\r\n")).unwrap_or_default();
        let encoded = body.to_string();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{encoded}",
            encoded.len()
        )?;
        stream.flush()?;
        let mut answer = String::new();
        BufReader::new(stream).read_to_string(&mut answer)?;
        let (head, body) = answer.split_once("\r\n\r\n").ok_or("the control answer has no body")?;
        let status: u16 = head.split_whitespace().nth(1).and_then(|code| code.parse().ok()).ok_or("the control answer has no status")?;
        Ok((status, serde_json::from_str(body)?))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // SIGKILL on Unix: no shutdown hook, no flush, an unclean stop.
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

/// The key the cabinet contract derives from the payload, never from the
/// attempt: a retry — or a restart — of the same base sends the same one.
fn derived_key(posted: &Posted) -> String {
    let version = posted.body["version"].as_u64().unwrap_or(0);
    let sha256 = posted.body["sha256"].as_str().unwrap_or_default();
    format!("{DOC}:{version}:{sha256}")
}

fn assert_delivery(posted: &Posted, version: u64, expected: u64) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(posted.body["version"], version, "{:?}", posted.body);
    assert_eq!(posted.body["expectedVersion"], expected, "{:?}", posted.body);
    assert_eq!(posted.body["idempotencyKey"], derived_key(posted), "the key stays derived from the payload");
    assert_eq!(posted.idempotency_key.as_deref(), Some(derived_key(posted).as_str()), "the header and the body agree");
    let sha256 = posted.body["sha256"].as_str().unwrap_or_default();
    assert_eq!(sha256.len(), 64, "a sha256 in hex: {sha256}");
    Ok(())
}

/// The daemon is killed after a commit whose POST failed retryably: the base is
/// on disk, the cabinet never took it, and the queue that would have retried it
/// died with the process.
fn abandoned_publication(root: &Path, data: &Path, public_pem: &Path, token: &str) -> Result<(), Box<dyn std::error::Error>> {
    let down = Cabinet::spawn(0, true)?;
    let daemon = Daemon::spawn(root, data, public_pem, &down.url)?;
    let (status, reply) = daemon.session(token, &fixture())?;
    assert_eq!(status, 201, "{reply}");
    let mut client = daemon.join(token)?;
    assert_eq!((client.version, client.epoch), (0, 0));
    client.apply(op("restore-1", note("restore-1", "kept"), 0, 0)?)?;
    match client.commit()? {
        ServerMessage::Error(error) => assert!(error.retryable, "a cabinet that is down is a retryable failure: {error:?}"),
        other => return Err(format!("expected the failed publication, got {other:?}").into()),
    }
    assert!(!down.posted().is_empty(), "the commit did try the cabinet");
    // The process goes away with the room `publishing` and its base on disk.
    drop(daemon);
    Ok(())
}

#[test]
fn a_restart_publishes_the_base_the_previous_process_left() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("pdfcraft-collab-restart-publish-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    std::fs::create_dir_all(&data)?;
    let key = key()?;
    let public_pem = root.join("cabinet-public.pem");
    std::fs::write(&public_pem, key.public_pem.as_bytes())?;
    let token = token(&key, "ada", "editor")?;

    abandoned_publication(&root, &data, &public_pem, &token)?;

    // The second process finds the room in the store — no `--documents`, no
    // session, no client — and publishes the base at once.
    let cabinet = Cabinet::spawn(0, false)?;
    let daemon = Daemon::spawn(&root, &data, &public_pem, &cabinet.url)?;
    let posted = cabinet.wait_for(1, Duration::from_secs(20)).map_err(|e| format!("{e}\n{}", daemon.died()))?;
    assert_eq!(posted.len(), 1, "one restored publication, not a queue of them");
    assert_delivery(&posted[0], 1, 0)?;
    assert_eq!(cabinet.held(), 1, "the cabinet stored one version for the base");

    let metrics = daemon.get("/metrics")?;
    assert!(metrics.contains("pdfcraft_collab_publications_restored_total 1"), "{metrics}");
    assert!(metrics.contains("pdfcraft_collab_publications_after_resolve_total 0"), "{metrics}");
    assert!(metrics.contains("pdfcraft_collab_publication_retries_total 0"), "it succeeded on the first attempt: {metrics}");
    assert!(metrics.contains(r#"pdfcraft_collab_room_version{room="file-1"} 1"#), "{metrics}");
    let health = daemon.get("/health")?;
    assert!(health.contains(r#""conflict":[]"#), "{health}");

    // A success is not queued again, and the attempt is not a background retry.
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(cabinet.posted().len(), 1, "the restored publication ran once: {}", cabinet.posted().len());

    // The marker moved: the next commit expects the version the cabinet gave
    // the restored base, so the cabinet saw two versions, not three.
    let mut client = daemon.join(&token)?;
    assert_eq!(client.version, 1, "the room came back at the base the previous process wrote");
    client.apply(op("restore-2", note("restore-2", "after"), 1, 0)?)?;
    match client.commit()? {
        ServerMessage::CommitResult(result) => assert_eq!(result.version, 2),
        other => return Err(format!("expected a commit result after the restored publication, got {other:?}").into()),
    }
    let posted = cabinet.wait_for(2, Duration::from_secs(10)).map_err(|e| e.to_string())?;
    assert_delivery(&posted[1], 2, 1)?;
    assert_eq!(cabinet.held(), 2);

    drop(daemon);
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

#[test]
fn a_restored_publication_the_cabinet_refuses_stops_and_a_resolve_publishes_it() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("pdfcraft-collab-restart-resolve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    std::fs::create_dir_all(&data)?;
    let key = key()?;
    let public_pem = root.join("cabinet-public.pem");
    std::fs::write(&public_pem, key.public_pem.as_bytes())?;
    let token = token(&key, "ada", "editor")?;

    abandoned_publication(&root, &data, &public_pem, &token)?;

    // The cabinet moved on while the daemon was down: the restored publication
    // names `expectedVersion: 0` and is refused.
    let cabinet = Cabinet::spawn(5, false)?;
    let daemon = Daemon::spawn(&root, &data, &public_pem, &cabinet.url)?;
    let posted = cabinet.wait_for(1, Duration::from_secs(20)).map_err(|e| format!("{e}\n{}", daemon.died()))?;
    assert_delivery(&posted[0], 1, 0)?;
    assert_eq!(cabinet.held(), 5, "a refused publication does not move the cabinet");
    daemon.wait_for_health(r#""conflict":["file-1"]"#, Duration::from_secs(10)).map_err(|e| format!("{e}\n{}", daemon.died()))?;

    let metrics = daemon.get("/metrics")?;
    assert!(metrics.contains("pdfcraft_collab_publications_restored_total 1"), "{metrics}");
    assert!(metrics.contains(r#"pdfcraft_collab_room_conflict{room="file-1"} 1"#), "{metrics}");
    assert!(metrics.contains("pdfcraft_collab_publication_retries_total 0"), "a 409 is never queued: {metrics}");
    // A `409` stops the queue: nothing tries the same expectation again.
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(cabinet.posted().len(), 1, "the refused publication was not retried: {}", cabinet.posted().len());

    // The operator says the cabinet holds version 5; the base at version 1 is
    // published now, with no commit, join or leave.
    let (status, reply) = daemon.resolve(Some(&token), &json!({"expectedVersion": 5, "etag": format!("sha256:{:064x}", 5)}))?;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["conflict"], false);
    let posted = cabinet.wait_for(2, Duration::from_secs(10)).map_err(|e| format!("{e}\n{}", daemon.died()))?;
    assert_eq!(posted.len(), 2, "the resolve published the base once");
    assert_delivery(&posted[1], 1, 5)?;
    assert_eq!(posted[1].body["sha256"], posted[0].body["sha256"], "the same base, so the same derived key");
    assert_eq!(posted[1].idempotency_key, posted[0].idempotency_key);
    assert_eq!(cabinet.held(), 6, "the cabinet stored one version for the resolved base");

    let metrics = daemon.get("/metrics")?;
    assert!(metrics.contains("pdfcraft_collab_publications_after_resolve_total 1"), "{metrics}");
    assert!(metrics.contains("pdfcraft_collab_conflicts_resolved_total 1"), "{metrics}");
    assert!(!metrics.contains("pdfcraft_collab_room_conflict"), "the room is not flagged any more: {metrics}");
    let health = daemon.get("/health")?;
    assert!(health.contains(r#""conflict":[]"#), "{health}");

    // The marker moved to what the cabinet answered: the next commit expects 6.
    let mut client = daemon.join(&token)?;
    client.apply(op("restore-2", note("restore-2", "after"), 1, 0)?)?;
    match client.commit()? {
        ServerMessage::CommitResult(result) => assert_eq!(result.version, 2),
        other => return Err(format!("expected a commit result after the resolve, got {other:?}").into()),
    }
    let posted = cabinet.wait_for(3, Duration::from_secs(10)).map_err(|e| e.to_string())?;
    assert_delivery(&posted[2], 2, 6)?;

    drop(daemon);
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
