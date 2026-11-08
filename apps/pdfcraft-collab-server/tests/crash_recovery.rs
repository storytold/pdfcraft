//! Crash recovery against the real daemon (RFC §8.1; `docs/collab/STATUS.md`
//! §5, limitation 1).
//!
//! The other crash tests cut a `Store` and a `Room` apart in-process, so the
//! process boundary — an accepted operation, a commit that has not written its
//! base yet, and a `SIGKILL` in the middle — is never exercised. This test runs
//! `pdfcraft-collab-server`, edits over a socket, and kills the process in the
//! window between the acknowledged journal record and the base write. The
//! restart must rebuild the head, the epoch and the `opId` window from the
//! journal: the operation is neither lost nor applied twice.
//!
//! The window is made deterministic by a fault point in `PdfAdapter::commit`
//! (`crates/collab-pdf/src/store.rs`, `park_at_fault`): debug builds park there
//! when `PDFCRAFT_COLLAB_FAULT=commit-before-base` is set, as long as the
//! journal record already exists, the test writes the marker file and kills the
//! process with `Child::kill` (SIGKILL on Unix). A release build compiles the
//! point out, so the test reports that it did not run instead of pretending.

use pdfcraft_collab_pdf::PdfAdapter;
use pdfcraft_collab_proto::{ClientMessage, CommitRequest, ConflictClass, DocKey, Op, OpId, OpKind, PageRef, ServerMessage, Target};
use serde::Serialize;
use serde_json::{Value, json};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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

struct Daemon {
    child: Child,
    addr: SocketAddr,
    stderr: PathBuf,
}

impl Daemon {
    fn spawn(root: &Path, documents: &Path, data: &Path, public_pem: &Path, fault: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let listen = free_port()?;
        let health = free_port()?;
        let stderr = root.join(format!("daemon-{listen}.stderr"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_pdfcraft-collab-server"));
        command
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
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&stderr)?);
        if let Some(marker) = fault {
            command.env("PDFCRAFT_COLLAB_FAULT", "commit-before-base").env("PDFCRAFT_COLLAB_FAULT_MARKER", marker);
        }
        let child = command.spawn()?;
        let addr: SocketAddr = format!("127.0.0.1:{listen}").parse()?;
        Ok(Self { child, addr, stderr })
    }

    fn died(&self) -> String {
        std::fs::read_to_string(&self.stderr).unwrap_or_default()
    }

    /// Wait until the daemon parks in the crash window the marker names.
    fn wait_for_fault(&mut self, marker: &Path, limit: Duration) -> Result<(), Box<dyn std::error::Error>> {
        let start = Instant::now();
        while start.elapsed() < limit {
            if marker.exists() {
                return Ok(());
            }
            if let Some(status) = self.child.try_wait()? {
                return Err(format!("the daemon exited ({status}) before the crash window:\n{}", self.died()).into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err(format!("the daemon did not reach the crash window within {limit:?}:\n{}", self.died()).into())
    }

    /// SIGKILL and reap (`Child::kill` is SIGKILL on Unix).
    fn kill(&mut self) -> Result<std::process::ExitStatus, Box<dyn std::error::Error>> {
        self.child.kill()?;
        Ok(self.child.wait()?)
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
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        }
        ws.send(Message::text(
            json!({
                "type": "hello",
                "v": 1,
                "app": "pdfcraft",
                "doc": DOC,
                "jwt": token,
                "client": {"name": "crash-test", "version": "0"},
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

    fn send(&mut self, message: &ClientMessage) -> Result<(), Box<dyn std::error::Error>> {
        self.ws.send(Message::text(serde_json::to_string(message)?))?;
        Ok(())
    }

    /// Send an operation and return what the server accepted or refused.
    fn apply(&mut self, op: Op) -> Result<pdfcraft_collab_proto::Applied, Box<dyn std::error::Error>> {
        self.send(&ClientMessage::Op(op))?;
        loop {
            match self.next()? {
                ServerMessage::Applied(applied) => {
                    self.version = applied.version;
                    self.epoch = applied.epoch;
                    return Ok(applied);
                }
                ServerMessage::Rejected(rejected) => return Err(format!("the operation was refused: {rejected:?}").into()),
                ServerMessage::Presence(_) => {}
                other => return Err(format!("expected an applied, got {other:?}").into()),
            }
        }
    }

    fn commit(&mut self) -> Result<pdfcraft_collab_proto::CommitResult, Box<dyn std::error::Error>> {
        self.send(&ClientMessage::Commit(CommitRequest { expected_version: self.version, message: None }))?;
        loop {
            match self.next()? {
                ServerMessage::CommitResult(result) => return Ok(result),
                ServerMessage::Presence(_) => {}
                other => return Err(format!("expected a commit result, got {other:?}").into()),
            }
        }
    }
}

fn op(id: &str, payload: Value, class: ConflictClass, target: Target, base_version: u64, epoch: u64) -> Result<Op, Box<dyn std::error::Error>> {
    Ok(Op {
        op_id: OpId::new(id)?,
        doc: DocKey::new(DOC)?,
        base_version,
        epoch,
        kind: match class {
            ConflictClass::Structural => OpKind::Structural,
            _ => OpKind::Primitive,
        },
        class,
        target,
        payload,
        preview: None,
    })
}

fn page(index: u32, epoch: u64) -> Target {
    Target::Page(PageRef { index, epoch })
}

fn note(nm: &str, contents: &str) -> Value {
    // The legacy sticky-note payload: name (`nm`) and text, no typed shape.
    json!({"action": "annotation.add", "at": [40.0, 250.0], "contents": contents, "nm": nm})
}

fn journal_lines(path: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    Ok(text.lines().filter(|line| !line.trim().is_empty()).map(str::to_owned).collect())
}

#[test]
fn a_sigkill_between_the_journal_and_the_base_loses_nothing_and_duplicates_nothing() -> Result<(), Box<dyn std::error::Error>> {
    if !cfg!(debug_assertions) {
        eprintln!("skipped: the commit-before-base fault point is compiled out of a release build");
        return Ok(());
    }
    let root = std::env::temp_dir().join(format!("pdfcraft-collab-crash-{}", std::process::id()));
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
    let marker = root.join("fault-reached");
    let room = data.join(DOC);

    // 1. A baseline head of two accepted operations, then a commit that dies.
    {
        let mut daemon = Daemon::spawn(&root, &documents, &data, &public_pem, Some(&marker))?;
        let mut client = daemon.join(&token)?;
        assert_eq!((client.version, client.epoch), (0, 0), "the seeded base is version 0");

        let first = client.apply(op("crash-a1", note("crash-a1", "kept"), ConflictClass::Merge, page(0, 0), 0, 0)?)?;
        assert_eq!((first.version, first.epoch), (1, 0));

        // A structural operation advances the epoch: the restart has to bring
        // back both the head and the epoch, not just the version.
        let rotate = json!({"action": "page.rotate", "degrees": 90});
        let second = client.apply(op("crash-r1", rotate, ConflictClass::Structural, page(0, 0), 1, 0)?)?;
        assert_eq!((second.version, second.epoch), (2, 1));

        // The commit snapshots the document; the daemon parks just before the
        // base is written, so the kill lands between the journal and the base.
        client.send(&ClientMessage::Commit(CommitRequest { expected_version: 2, message: None }))?;
        daemon.wait_for_fault(&marker, Duration::from_secs(20))?;

        // What is on disk at the moment of the kill: both accepted operations
        // in the journal, no base for version 2, CURRENT still naming version 0.
        let lines = journal_lines(&room.join("journal.jsonl"))?;
        assert_eq!(lines.len(), 2, "both acknowledged operations are durable: {lines:?}");
        assert!(lines[0].contains(r#""version":1"#) && lines[0].contains("crash-a1"), "{:?}", lines[0]);
        assert!(lines[1].contains(r#""version":2"#) && lines[1].contains("crash-r1"), "{:?}", lines[1]);
        assert!(!room.join("versions/2").exists(), "the base the commit was about to write is not there");
        assert_eq!(std::fs::read_to_string(room.join("CURRENT"))?, r#"{"version":0,"epoch":0}"#);

        let status = daemon.kill()?;
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(9), "the daemon was killed by SIGKILL");
        }
        #[cfg(not(unix))]
        let _ = status;
    }

    // 2. A restart replays the journal onto the base the daemon still had.
    let daemon = Daemon::spawn(&root, &documents, &data, &public_pem, None)?;
    let mut client = daemon.join(&token)?;
    assert_eq!((client.version, client.epoch), (2, 1), "the head and the epoch came back from the journal");

    // The retried operation returns the stored result: same version, same
    // epoch, and no second copy in the document (that is checked on the base).
    let retried = client.apply(op("crash-a1", note("crash-a1", "kept"), ConflictClass::Merge, page(0, 0), 0, 0)?)?;
    assert_eq!(retried.op_id.as_str(), "crash-a1");
    assert_eq!((retried.version, retried.epoch), (1, 0), "a retry replays the acknowledgement, it does not apply again");

    // A new merge operation has to name the restored epoch, and the head must
    // be 2: a duplicated retry would have made this version 4.
    let after = client.apply(op("crash-a3", note("crash-a3", "after-restart"), ConflictClass::Merge, page(0, 1), 2, 1)?)?;
    assert_eq!((after.version, after.epoch), (3, 1), "the deduplicated retry did not advance the head");

    let result = client.commit()?;
    assert_eq!(result.version, 3);
    assert!(!result.sha256.is_empty());
    drop(daemon);

    // 3. The committed base holds every operation exactly once.
    assert_eq!(std::fs::read_to_string(room.join("CURRENT"))?, r#"{"version":3,"epoch":1}"#);
    assert!(journal_lines(&room.join("journal.jsonl"))?.is_empty(), "the base absorbed the journal");
    let base = std::fs::read(room.join("versions/3/file.pdf"))?;
    let adapter = PdfAdapter::open("recovered.pdf", base.into())?;
    let document = adapter.document().ok_or("the committed base did not open")?;
    assert_eq!(document.info.pages[0].rotation, 90, "the structural operation survived the crash");
    let mut names: Vec<&str> = document.info.annotations.iter().filter_map(|a| a.name.as_deref()).collect();
    names.sort_unstable();
    assert_eq!(names, ["crash-a1", "crash-a3"], "both comments are there once, and the retried one is not duplicated");

    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
