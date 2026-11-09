//! The cabinet publication queue against the real daemon (RFC §8.2,
//! `docs/collab/STATUS.md` §5, limitation 6).
//!
//! A commit writes its base locally and then POSTs the file to the cabinet
//! callback. When that POST fails retryably — here the cabinet takes the bytes
//! and the connection dies before the answer, the classic at-least-once window
//! — the daemon has to try again by itself, with the same body and the same
//! idempotency key, instead of waiting for the next commit or the last client
//! leaving.
//!
//! The cabinet in this test keeps the version it accepted, recognises the
//! second delivery by its `Idempotency-Key`, and answers `200` with
//! `{"duplicate":true}`. The daemon must then hold one version, not two: the
//! retry is a duplicate of the same publication, and the next commit's
//! `expectedVersion` proves the marker advanced.

use pdfcraft_collab_proto::{ClientMessage, CommitRequest, ConflictClass, DocKey, Op, OpId, OpKind, PageRef, ServerMessage, Target};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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

/// What one callback request carried.
#[derive(Debug, Clone)]
struct Posted {
    idempotency_key: Option<String>,
    body: Value,
}

/// The cabinet's commit endpoint. The first delivery is accepted and the
/// connection is closed without an answer (the response was lost); every later
/// delivery of the same version is a duplicate, which the cabinet answers `2xx`
/// without creating a second version.
struct Cabinet {
    url: String,
    posted: Arc<Mutex<Vec<Posted>>>,
    versions: Arc<Mutex<Vec<u64>>>,
}

impl Cabinet {
    fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let posted = Arc::new(Mutex::new(Vec::new()));
        let versions = Arc::new(Mutex::new(Vec::new()));
        let seen = posted.clone();
        let stored = versions.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if handle(stream, &seen, &stored).is_err() {
                    return;
                }
            }
        });
        Ok(Self { url: format!("http://{address}/commit"), posted, versions })
    }

    fn posted(&self) -> Vec<Posted> {
        self.posted.lock().map(|posted| posted.clone()).unwrap_or_default()
    }

    fn versions(&self) -> Vec<u64> {
        self.versions.lock().map(|versions| versions.clone()).unwrap_or_default()
    }
}

fn handle(mut stream: TcpStream, posted: &Mutex<Vec<Posted>>, versions: &Mutex<Vec<u64>>) -> Result<(), Box<dyn std::error::Error>> {
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
    let version = body["version"].as_u64().unwrap_or(0);
    let key = body["idempotencyKey"].as_str().map(str::to_owned);
    assert_eq!(key, idempotency_key, "the body and the header carry the same key");

    let duplicate = {
        let mut stored = versions.lock().map_err(|_| "the cabinet lock is poisoned")?;
        if stored.contains(&version) {
            true
        } else {
            stored.push(version);
            false
        }
    };
    posted.lock().map_err(|_| "the cabinet lock is poisoned")?.push(Posted { idempotency_key, body });

    if !duplicate && posted.lock().map(|posted| posted.len()).unwrap_or(0) == 1 {
        // Accepted, but the answer is lost: the daemon sees a failed call and
        // will retry the same key.
        return Ok(());
    }
    let answer = if duplicate { json!({"version": version, "duplicate": true}) } else { json!({"version": version}) };
    let answer = answer.to_string();
    let _ =
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
    let _ = stream.flush();
    Ok(())
}

struct Daemon {
    child: Child,
    addr: SocketAddr,
    health: SocketAddr,
    stderr: PathBuf,
}

impl Daemon {
    fn spawn(root: &Path, documents: &Path, data: &Path, public_pem: &Path, callback: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let listen = free_port()?;
        let health = free_port()?;
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
            .arg("--callback-url")
            .arg(callback)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&stderr)?)
            .spawn()?;
        Ok(Self { child, addr: format!("127.0.0.1:{listen}").parse()?, health: format!("127.0.0.1:{health}").parse()?, stderr })
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
                "client": {"name": "retry-test", "version": "0"},
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

#[test]
fn a_lost_callback_answer_is_retried_with_the_same_key_and_one_version() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("pdfcraft-collab-retry-{}", std::process::id()));
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
    let cabinet = Cabinet::start()?;

    let daemon = Daemon::spawn(&root, &documents, &data, &public_pem, &cabinet.url)?;
    let mut client = daemon.join(&token)?;
    assert_eq!((client.version, client.epoch), (0, 0));

    client.apply(op("retry-1", note("retry-1", "kept"), 0, 0)?)?;
    // The cabinet takes the bytes and the answer is lost: the commit reports a
    // retryable failure, and the queue has to try again by itself.
    match client.commit()? {
        ServerMessage::Error(error) => assert!(error.retryable, "a lost answer is a retryable failure: {error:?}"),
        other => return Err(format!("expected the failed publication, got {other:?}").into()),
    }

    // The daemon waits one second before the first retry; give it a few.
    let deadline = Instant::now() + Duration::from_secs(20);
    while cabinet.posted().len() < 2 {
        if Instant::now() > deadline {
            return Err(format!("the daemon did not retry the callback ({} delivery/ies)\n{}", cabinet.posted().len(), daemon.died()).into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let posted = cabinet.posted();
    assert_eq!(posted.len(), 2, "one failed attempt and one retry, no third: {posted:?}");
    let first = &posted[0];
    let second = &posted[1];
    assert_eq!(first.body["version"], 1);
    assert_eq!(first.body["expectedVersion"], 0);
    assert_eq!(second.body["version"], 1, "the retry publishes the same version, not a new one");
    assert_eq!(second.body["expectedVersion"], 0);
    assert_eq!(second.body["sha256"], first.body["sha256"]);
    assert_eq!(second.idempotency_key, first.idempotency_key, "the retry carries the same idempotency key");
    assert_eq!(second.body["idempotencyKey"], first.body["idempotencyKey"]);
    assert_eq!(cabinet.versions(), vec![1], "the cabinet stored one version, the duplicate was not a second one");

    // The metric counts the background attempts, and a transport failure did
    // not turn into a conflict.
    let metrics = daemon.get("/metrics")?;
    assert!(metrics.contains("pdfcraft_collab_publication_retries_total 1"), "{metrics}");
    let health = daemon.get("/health")?;
    assert!(health.contains(r#""conflict":[]"#), "{health}");

    // The retry advanced what the cabinet is believed to hold: the next commit
    // expects version 1, so the marker moved without a second version.
    client.apply(op("retry-2", note("retry-2", "after"), 1, 0)?)?;
    match client.commit()? {
        ServerMessage::CommitResult(result) => assert_eq!(result.version, 2),
        other => return Err(format!("expected a commit result after the retry, got {other:?}").into()),
    }
    let posted = cabinet.posted();
    let third = posted.last().ok_or("the second commit was not delivered")?;
    assert_eq!((third.body["version"].as_u64(), third.body["expectedVersion"].as_u64()), (Some(2), Some(1)));
    assert_eq!(cabinet.versions(), vec![1, 2], "two commits, two versions: the retry added none");

    drop(daemon);
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
