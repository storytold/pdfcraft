//! Rooms and the cabinet contract: how a document becomes a room, how a commit
//! is published to the cabinet, and when a commit is refused because the file
//! changed outside the session (RFC §8, §9).
//!
//! Two ways in:
//! - `seed_rooms` opens one room per `<fileId>.pdf` in a directory. That is the
//!   dev mode: no cabinet, the source file is never written, and a change to it
//!   is detected locally by hashing.
//! - the control plane (`POST /v1/sessions`) opens a room from bytes or a URL
//!   supplied by the cabinet, after the room's etag check.
//!
//! A commit writes the base locally first, then publishes the file to the
//! cabinet (`{fileId, bytes, sha256, version, expectedVersion}`); only an
//! accepted publication advances what the cabinet is known to hold.

use crate::http::{Client, HttpError};
use pdfcraft_collab_hub::Hub;
use pdfcraft_collab_pdf::{CabinetCopy, PdfAdapter, Store};
use pdfcraft_collab_proto::{ActorId, BlobRef, DocKey, ErrorCode};
use pdfcraft_collab_server::{Adapter, AdapterError, ApplyOutcome, CommitOutcome, Publication, PublishOutcome, Published};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How a room reaches the cabinet and how big a source file may be.
#[derive(Clone)]
pub struct RoomsConfig {
    pub data: PathBuf,
    pub publisher: Option<Arc<Publisher>>,
    pub max_source_bytes: usize,
    /// Deadline for fetching a source URL.
    pub http_timeout: Duration,
}

/// The cabinet's commit endpoint.
pub struct Publisher {
    url: String,
    token: Option<String>,
    client: Client,
}

#[derive(Debug)]
pub enum PublishError {
    /// The cabinet has a version this session did not start from.
    Conflict(String),
    /// The cabinet or the network failed, and another attempt may work: the
    /// daemon's queue retries it after a backoff with the same body and the
    /// same idempotency key.
    Retryable(String),
    /// Anything else (a `4xx` that a retry would get again): the file is
    /// committed locally but the cabinet did not take it, and the next commit
    /// retries it.
    Failed(String),
}

impl Publisher {
    pub fn new(url: impl Into<String>, token: Option<String>, timeout: Duration) -> Self {
        Self { url: url.into(), token, client: Client { timeout } }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Send the committed file. The body is the contract of RFC §8:
    /// `{fileId, bytes, sha256, version, expectedVersion, idempotencyKey}`, with
    /// the file base64-encoded. `expectedVersion` is the version the cabinet is
    /// believed to hold, so the cabinet can refuse with `409` when it moved on.
    ///
    /// The idempotency key names the payload (`fileId:version:sha256`), not the
    /// attempt: a retry after a lost response sends the same key, so a cabinet
    /// that already took the version recognises the duplicate instead of
    /// creating a second one. The same value travels in the body and in the
    /// `Idempotency-Key` header.
    pub fn publish(&self, file_id: &str, version: u64, expected_version: u64, bytes: &[u8], sha256: &str) -> Result<(), PublishError> {
        let key = idempotency_key(file_id, version, sha256);
        let body = serde_json::json!({
            "fileId": file_id,
            "bytes": base64_encode(bytes),
            "sha256": sha256,
            "version": version,
            "expectedVersion": expected_version,
            "idempotencyKey": key,
        })
        .to_string();
        let headers = [("Idempotency-Key", key.as_str())];
        match self.client.post_json(&self.url, self.token.as_deref(), &headers, body.as_bytes()).map(|(status, _)| status) {
            Ok(status) if (200..300).contains(&status) => Ok(()),
            Ok(409) => Err(PublishError::Conflict("the cabinet holds another version of the file".into())),
            // The cabinet is busy, a proxy timed out, or the file will be there
            // next time: another attempt with the same key may succeed.
            Ok(status @ 429 | status @ 500..=599) => Err(PublishError::Retryable(format!("the cabinet answered {status}"))),
            Ok(status) => Err(PublishError::Failed(format!("the cabinet answered {status}"))),
            Err(HttpError(message)) => Err(PublishError::Retryable(message)),
        }
    }
}

/// What an idempotent callback is keyed by: the file, the version and the bytes.
/// Deterministic, so a retry after a restart (or a lost response) is the same
/// key and cannot become a second version.
pub fn idempotency_key(file_id: &str, version: u64, sha256: &str) -> String {
    format!("{file_id}:{version}:{sha256}")
}

/// A room's adapter plus everything the cabinet contract adds to it: the
/// external-change check, the publication and the version the cabinet holds.
pub struct CabinetAdapter {
    pdf: PdfAdapter,
    file_id: String,
    /// The cabinet's copy in dev mode: a room seeded from a directory is
    /// refused a commit when that file changed under it.
    mirror: Option<PathBuf>,
    publisher: Option<Arc<Publisher>>,
    expected_version: u64,
    /// The publication the last `commit` left for the transport to run outside
    /// the hub lock.
    publication: Option<Box<dyn Publication>>,
}

/// The cabinet POST, taken out of the room so it runs without the hub lock.
struct CabinetPublication {
    publisher: Arc<Publisher>,
    file_id: String,
    version: u64,
    expected_version: u64,
    bytes: Arc<Vec<u8>>,
    sha256: String,
}

impl Publication for CabinetPublication {
    fn run(&self) -> PublishOutcome {
        match self.publisher.publish(&self.file_id, self.version, self.expected_version, self.bytes.as_slice(), &self.sha256) {
            Ok(()) => Ok(Some(Published { version: self.version, etag: format!("sha256:{}", self.sha256) })),
            Err(PublishError::Conflict(message)) => Err(AdapterError::new(ErrorCode::Conflict, message, false)),
            // The room keeps `publishing` and the queue tries the same body and
            // key again after a backoff (limitation 6).
            Err(PublishError::Retryable(message)) => {
                Err(AdapterError::new(ErrorCode::Internal, format!("the commit is saved locally but not published: {message}"), true))
            }
            Err(PublishError::Failed(message)) => {
                Err(AdapterError::new(ErrorCode::Internal, format!("the commit is saved locally but not published: {message}"), false))
            }
        }
    }
}

impl CabinetAdapter {
    pub fn new(
        pdf: PdfAdapter,
        file_id: impl Into<String>,
        mirror: Option<PathBuf>,
        publisher: Option<Arc<Publisher>>,
        expected_version: u64,
    ) -> Self {
        Self { pdf, file_id: file_id.into(), mirror, publisher, expected_version, publication: None }
    }

    /// The PDF adapter underneath, for inspection by tests.
    #[cfg(test)]
    pub fn pdf(&self) -> &PdfAdapter {
        &self.pdf
    }

    /// Refuse a commit when the file the room was seeded from changed. The etag
    /// is the SHA-256 of the file as seeded, so a rewritten file (a cabinet
    /// upload while the session was live) is caught before the base is written.
    fn check_mirror(&self) -> Result<(), AdapterError> {
        let (Some(path), Some(published)) = (&self.mirror, Adapter::published(self)) else { return Ok(()) };
        let now = etag_of_file(path).map_err(|e| AdapterError::new(ErrorCode::Invalid, e, false))?;
        if now != published.etag {
            return Err(AdapterError::new(
                ErrorCode::Conflict,
                format!("{} changed outside the session; save a copy instead of committing", path.display()),
                false,
            ));
        }
        Ok(())
    }
}

impl Adapter for CabinetAdapter {
    fn apply(&mut self, actor: &ActorId, op: &pdfcraft_collab_proto::Op) -> Result<ApplyOutcome, AdapterError> {
        self.pdf.apply(actor, op)
    }

    fn record(&mut self, version: u64, actor: &ActorId, op: &pdfcraft_collab_proto::Op) -> Result<(), AdapterError> {
        self.pdf.record(version, actor, op)
    }

    fn commit(&mut self, version: u64, epoch: u64) -> Result<CommitOutcome, AdapterError> {
        self.check_mirror()?;
        let outcome = self.pdf.commit(version, epoch)?;
        // The cabinet call is not made here: it is handed to the transport,
        // which runs it after releasing the hub lock (RFC §16).
        self.publication = match &self.publisher {
            Some(publisher) => {
                let bytes = self
                    .pdf
                    .last_commit()
                    .cloned()
                    .ok_or_else(|| AdapterError::new(ErrorCode::Internal, "the committed file is not available for publication", false))?;
                Some(Box::new(CabinetPublication {
                    publisher: publisher.clone(),
                    file_id: self.file_id.clone(),
                    version,
                    expected_version: self.expected_version,
                    bytes,
                    sha256: outcome.sha256.clone(),
                }))
            }
            None => None,
        };
        Ok(outcome)
    }

    fn take_publication(&mut self) -> Option<Box<dyn Publication>> {
        self.publication.take()
    }

    fn finish_publish(&mut self, version: u64, outcome: PublishOutcome) -> Result<(), AdapterError> {
        let etag = match outcome {
            // The cabinet took the bytes: that etag is what it holds.
            Ok(Some(published)) if self.mirror.is_none() => published.etag,
            // Dev mode keeps the mirror file as the etag's reference, and with
            // no publisher the local commit is all the publication there is.
            Ok(_) => Adapter::published(self).map(|published| published.etag).unwrap_or_default(),
            // A refused or failed publication leaves the marker alone; the
            // room stays dirty so the next commit retries it.
            Err(_) => return Ok(()),
        };
        self.pdf.set_published(Some(Published { version, etag }))?;
        self.expected_version = version;
        Ok(())
    }

    fn base(&self) -> Option<BlobRef> {
        self.pdf.base()
    }

    fn published(&self) -> Option<Published> {
        Adapter::published(&self.pdf)
    }

    fn set_published(&mut self, published: Option<Published>) -> Result<(), AdapterError> {
        self.pdf.set_published(published)
    }
}

/// A room opened or resumed from a known file.
#[derive(Debug)]
pub struct Opened {
    pub version: u64,
    pub epoch: u64,
    /// `true` when this call created the room, `false` when it resumed one.
    pub created: bool,
    /// The etag of the cabinet's copy, when the room knows it.
    pub etag: Option<String>,
}

pub struct Seed {
    pub bytes: Arc<Vec<u8>>,
    pub etag: String,
    pub mirror: Option<PathBuf>,
}

#[derive(Debug)]
pub enum RoomError {
    /// The cabinet's copy is not the one the room holds. `etag` and `version`
    /// describe what the room has, so the caller can tell the cabinet.
    Conflict { message: String, etag: Option<String>, version: u64 },
    /// There is no room and no file to start one from.
    Missing(String),
    /// The file or the room directory cannot be used.
    Invalid(String),
}

impl RoomError {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

impl std::fmt::Display for RoomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { message, .. } => f.write_str(message),
            Self::Missing(message) | Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for RoomError {}

impl From<AdapterError> for RoomError {
    fn from(error: AdapterError) -> Self {
        match error.code {
            ErrorCode::Conflict => Self::Conflict { message: error.message, etag: None, version: 0 },
            _ => Self::Invalid(error.message),
        }
    }
}

/// A file id becomes a directory name, so it must not be able to leave `--data`.
pub fn doc_key(file_id: &str) -> Result<DocKey, String> {
    if file_id.is_empty() || file_id.len() > 128 {
        return Err("fileId must be between 1 and 128 bytes".into());
    }
    if file_id == "." || file_id == ".." {
        return Err("fileId must name a file".into());
    }
    if !file_id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        return Err("fileId may contain only letters, digits, '.', '-' and '_'".into());
    }
    DocKey::new(file_id).map_err(|e| e.to_string())
}

/// The etag of a file: `sha256:<hex>`.
pub fn etag_of(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

pub fn etag_of_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    Ok(etag_of(&bytes))
}

/// Open a room or resume it from its durable state.
///
/// `seed` is the file the cabinet has. It is used to create a room, and its
/// etag is compared with the room's when resuming: a different etag means the
/// file changed outside the session, and the room is not resumed (RFC §8).
pub fn open_room(hub: &mut Hub<CabinetAdapter>, config: &RoomsConfig, doc: DocKey, seed: Option<Seed>) -> Result<Opened, RoomError> {
    let dir = config.data.join(doc.as_str());
    let mut store = Store::open(dir)?;
    let (mut version, mut epoch) = store.base_state();
    let records = store.journal()?;
    let base = store.base()?;
    let seed = seed.map(|seed| (seed.bytes, seed.etag, seed.mirror));

    let (bytes, mirror, created) = match (base, seed) {
        (Some(base), seed) => {
            if let Some((_, etag, _)) = &seed {
                match store.cabinet() {
                    Some(copy) if &copy.etag != etag => {
                        return Err(RoomError::Conflict {
                            message: format!("the file changed outside the session: the room holds {}", copy.etag),
                            etag: Some(copy.etag.clone()),
                            version,
                        });
                    }
                    // A room seeded before the marker existed adopts the etag
                    // of the file it is resumed from.
                    None => store.set_cabinet(Some(CabinetCopy { version, etag: etag.clone() }))?,
                    Some(_) => {}
                }
            }
            let mirror = seed.and_then(|(_, _, mirror)| mirror);
            (base, mirror, false)
        }
        (None, Some((bytes, etag, mirror))) => {
            if bytes.len() > config.max_source_bytes {
                return Err(RoomError::invalid(format!("the file is larger than the {} byte limit", config.max_source_bytes)));
            }
            store.write_base(0, 0, bytes.as_slice())?;
            store.set_cabinet(Some(CabinetCopy { version: 0, etag }))?;
            version = 0;
            epoch = 0;
            (bytes.to_vec(), mirror, true)
        }
        (None, None) => return Err(RoomError::Missing(format!("no session and no file for {}", doc.as_str()))),
    };

    let published_version = store.cabinet().map_or(0, |copy| copy.version);
    let mut pdf = PdfAdapter::open(format!("{}.pdf", doc.as_str()), Arc::new(bytes))?;
    pdf.attach_store(store);
    let adapter = CabinetAdapter::new(pdf, doc.as_str(), mirror, config.publisher.clone(), published_version);
    hub.open_room_at(doc.clone(), adapter, version, epoch).map_err(|e| RoomError::invalid(e.message))?;
    let room = hub.room_mut(&doc).ok_or_else(|| RoomError::invalid("the room was not registered"))?;
    room.mark_published(published_version);
    for record in records.into_iter().filter(|record| record.version > version) {
        let actor = ActorId::new(record.actor.clone()).map_err(|e| RoomError::invalid(format!("journal actor {:?}: {e}", record.actor)))?;
        let replay_version = record.version;
        room.replay(actor, record.op, replay_version).map_err(|e| RoomError::invalid(e.message))?;
    }
    let etag = Adapter::published(room.adapter()).map(|published| published.etag);
    Ok(Opened { version: room.version(), epoch: room.epoch(), created, etag })
}

/// Dev mode: one room per `<fileId>.pdf` in `documents`. The source file is
/// never written; it becomes the etag the room is compared against.
pub fn seed_rooms(hub: &mut Hub<CabinetAdapter>, documents: &Path, config: &RoomsConfig) -> Result<usize, RoomError> {
    let entries = std::fs::read_dir(documents).map_err(|e| RoomError::invalid(format!("could not read {}: {e}", documents.display())))?;
    std::fs::create_dir_all(&config.data).map_err(|e| RoomError::invalid(format!("could not create {}: {e}", config.data.display())))?;
    let mut opened = 0;
    for entry in entries {
        let entry = entry.map_err(|e| RoomError::invalid(format!("could not read a document entry: {e}")))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pdf") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let doc = doc_key(stem).map_err(RoomError::invalid)?;
        let bytes = std::fs::read(&path).map_err(|e| RoomError::invalid(format!("could not read {}: {e}", path.display())))?;
        let seed = Seed { etag: etag_of(&bytes), bytes: Arc::new(bytes), mirror: Some(path.clone()) };
        open_room(hub, config, doc, Some(seed))?;
        opened += 1;
    }
    Ok(opened)
}

/// Standard base64 with padding. Kept here rather than pulled in as a
/// dependency: the wire format is fixed by the cabinet contract and the code is
/// a loop and a table.
pub fn base64_encode(bytes: &[u8]) -> String {
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

/// Decode standard base64. Whitespace is refused rather than skipped: a body
/// the cabinet sends is either the encoding of a file or an error.
pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    if !text.len().is_multiple_of(4) {
        return Err("base64 text must be a multiple of 4 characters".into());
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for (index, chunk) in text.as_bytes().chunks(4).enumerate() {
        let last = index == text.len() / 4 - 1;
        let pad = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return Err("base64 padding is in the wrong place".into());
        }
        let mut n: u32 = 0;
        for byte in &chunk[..4 - pad] {
            n = (n << 6) | u32::from(value(*byte).ok_or_else(|| "base64 text has a character outside the alphabet".to_owned())?);
        }
        n <<= 6 * pad as u32;
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..4 - pad]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{
        CabinetAdapter, PublishError, Publisher, RoomError, RoomsConfig, Seed, base64_decode, base64_encode, doc_key, etag_of, open_room, seed_rooms,
    };
    use pdfcraft_collab_hub::Hub;
    use pdfcraft_collab_proto::{ActorId, DocKey, ErrorCode, Role};
    use pdfcraft_collab_server::{Adapter, Room, RoomConfig};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;

    fn fixture(label: &str) -> Vec<u8> {
        let body = format!("BT /F1 18 Tf 20 150 Td ({label}) Tj ET");
        let objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes(),
        ];
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

    fn root(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("pdfcraft-collab-rooms-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn config(root: &std::path::Path, publisher: Option<Arc<Publisher>>) -> RoomsConfig {
        RoomsConfig { data: root.join("data"), publisher, max_source_bytes: 8 << 20, http_timeout: Duration::from_secs(5) }
    }

    fn hello() -> Result<pdfcraft_collab_proto::Hello, Box<dyn std::error::Error>> {
        Ok(pdfcraft_collab_proto::Hello {
            v: 1,
            app: "pdfcraft".into(),
            doc: DocKey::new("file-1")?,
            jwt: "t".into(),
            client: pdfcraft_collab_proto::ClientInfo { name: "test".into(), version: "0".into() },
        })
    }

    /// Commit the way the transport does: the base under the lock, the cabinet
    /// call outside it, the outcome recorded after.
    fn commit(
        room: &mut Room<CabinetAdapter>,
        actor: &ActorId,
        expected: u64,
    ) -> Result<pdfcraft_collab_proto::CommitResult, pdfcraft_collab_proto::ServerError> {
        let commit = room.commit(actor, expected)?;
        let outcome = commit.publish();
        room.finish(commit, outcome)
    }

    fn edit(room: &mut Room<CabinetAdapter>, nm: &str) -> Result<(), Box<dyn std::error::Error>> {
        let actor = ActorId::new("editor")?;
        let op = pdfcraft_collab_proto::Op {
            op_id: pdfcraft_collab_proto::OpId::new(nm)?,
            doc: DocKey::new("file-1")?,
            base_version: room.version(),
            epoch: room.epoch(),
            kind: pdfcraft_collab_proto::OpKind::Primitive,
            class: pdfcraft_collab_proto::ConflictClass::Merge,
            target: pdfcraft_collab_proto::Target::Page(pdfcraft_collab_proto::PageRef { index: 0, epoch: room.epoch() }),
            payload: serde_json::json!({"action": "annotation.add", "at": [40.0, 250.0], "contents": nm, "nm": nm}),
            preview: None,
        };
        room.apply(&actor, &op, 0)?;
        Ok(())
    }

    #[test]
    fn base64_round_trips_and_refuses_junk() {
        for case in [b"".as_slice(), b"a", b"ab", b"abc", b"%PDF-1.7 x"] {
            assert_eq!(base64_decode(&base64_encode(case)).as_deref(), Ok(case));
        }
        assert!(base64_decode("a").is_err());
        assert!(base64_decode("****").is_err());
        assert!(base64_decode("AB=C").is_err());
        assert_eq!(base64_decode("AA==").as_deref(), Ok(b"\0".as_slice()));
    }

    #[test]
    fn a_file_id_cannot_leave_the_data_directory() {
        assert!(doc_key("file-123").is_ok());
        assert!(doc_key("file_1.2").is_ok());
        assert!(doc_key("").is_err());
        assert!(doc_key("..").is_err());
        assert!(doc_key("../etc/passwd").is_err());
        assert!(doc_key("a/b").is_err());
        assert!(doc_key("file id").is_err());
    }

    #[test]
    fn a_changed_source_file_refuses_the_commit() -> Result<(), Box<dyn std::error::Error>> {
        let root = root("mirror");
        let documents = root.join("documents");
        std::fs::create_dir_all(&documents)?;
        let source = documents.join("file-1.pdf");
        std::fs::write(&source, fixture("one"))?;
        let config = config(&root, None);
        let mut hub = Hub::new(RoomConfig::default());
        assert_eq!(seed_rooms(&mut hub, &documents, &config)?, 1);

        let doc = DocKey::new("file-1")?;
        let room = hub.room_mut(&doc).ok_or("no room")?;
        let editor = ActorId::new("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        edit(room, "n0")?;
        assert!(commit(room, &editor, 1).is_ok(), "the seeded file is unchanged");

        // The cabinet uploaded a new version while the session was live.
        std::fs::write(&source, fixture("two"))?;
        edit(room, "n1")?;
        let refused = commit(room, &editor, 2).map(|_| ()).expect_err("the source changed");
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert!(room.conflict().is_some());
        assert!(room.is_dirty(), "nothing was published");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// A cabinet that answers one commit. Returns the address and the bodies it
    /// was sent.
    fn cabinet(status: u16) -> std::io::Result<(String, std::sync::mpsc::Receiver<serde_json::Value>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(match stream.try_clone() {
                    Ok(clone) => clone,
                    Err(_) => return,
                });
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                let _ = sender.send(serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null));
                let mut stream = match stream.try_clone() {
                    Ok(clone) => clone,
                    Err(_) => return,
                };
                let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
                let _ = stream.flush();
            }
        });
        Ok((format!("http://{address}/commit"), receiver))
    }

    #[test]
    fn a_commit_posts_the_file_and_the_versions() -> Result<(), Box<dyn std::error::Error>> {
        let root = root("publish");
        let (url, requests) = cabinet(200)?;
        let publisher = Arc::new(Publisher::new(url, None, Duration::from_secs(5)));
        let config = config(&root, Some(publisher));
        let doc = DocKey::new("file-1")?;
        let bytes = Arc::new(fixture("one"));
        let seed = Seed { etag: etag_of(&bytes), bytes: bytes.clone(), mirror: None };
        let mut hub = Hub::new(RoomConfig::default());
        let opened = open_room(&mut hub, &config, doc.clone(), Some(seed))?;
        assert!(opened.created);
        let room = hub.room_mut(&doc).ok_or("no room")?;
        let editor = ActorId::new("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        edit(room, "n0")?;
        let result = commit(room, &editor, 1)?;
        assert_eq!(result.version, 1);

        let body = requests.recv_timeout(Duration::from_secs(5))?;
        assert_eq!(body["fileId"], "file-1");
        assert_eq!(body["version"], 1);
        assert_eq!(body["expectedVersion"], 0);
        assert_eq!(body["sha256"], result.sha256);
        assert_eq!(
            body["idempotencyKey"],
            format!("file-1:1:{}", result.sha256),
            "the key names the payload, so a retry is a duplicate to the cabinet"
        );
        let posted = base64_decode(body["bytes"].as_str().ok_or("no bytes")?)?;
        assert!(posted.starts_with(b"%PDF-"), "the committed file is what is published");

        // The next commit expects the version the cabinet just took.
        edit(room, "n1")?;
        let second = commit(room, &editor, 2)?;
        let body = requests.recv_timeout(Duration::from_secs(5))?;
        assert_eq!((body["version"].as_u64(), body["expectedVersion"].as_u64()), (Some(2), Some(1)));
        let published = Adapter::published(room.adapter()).ok_or("no cabinet marker")?;
        assert_eq!((published.version, published.etag), (2, format!("sha256:{}", second.sha256)));
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn a_transport_failure_is_retryable_and_a_refused_body_is_not() -> Result<(), Box<dyn std::error::Error>> {
        let (url, requests) = cabinet(503)?;
        let publisher = Publisher::new(url, None, Duration::from_secs(5));
        match publisher.publish("file-1", 1, 0, b"%PDF-1.7 x", "abc") {
            Err(PublishError::Retryable(message)) => assert!(message.contains("503"), "{message}"),
            other => return Err(format!("a 503 has to be retryable, got {other:?}").into()),
        }
        let body = requests.recv_timeout(Duration::from_secs(5))?;
        assert_eq!(body["idempotencyKey"], "file-1:1:abc");

        // A 4xx that is not a conflict is the contract being wrong: another
        // attempt would be refused again, so it stays a plain failure.
        let (url, _requests) = cabinet(400)?;
        let publisher = Publisher::new(url, None, Duration::from_secs(5));
        assert!(matches!(publisher.publish("file-1", 1, 0, b"%PDF-1.7 x", "abc"), Err(PublishError::Failed(_))));

        // No cabinet at all: the connection is the failure, and it is retryable.
        let publisher = Publisher::new("http://127.0.0.1:1/commit", None, Duration::from_millis(200));
        assert!(matches!(publisher.publish("file-1", 1, 0, b"%PDF-1.7 x", "abc"), Err(PublishError::Retryable(_))));
        Ok(())
    }

    #[test]
    fn a_cabinet_that_reports_409_refuses_the_commit() -> Result<(), Box<dyn std::error::Error>> {
        let root = root("conflict");
        let (url, _requests) = cabinet(409)?;
        let publisher = Arc::new(Publisher::new(url, None, Duration::from_secs(5)));
        let config = config(&root, Some(publisher));
        let doc = DocKey::new("file-1")?;
        let bytes = Arc::new(fixture("one"));
        let seed = Seed { etag: etag_of(&bytes), bytes: bytes.clone(), mirror: None };
        let mut hub = Hub::new(RoomConfig::default());
        open_room(&mut hub, &config, doc.clone(), Some(seed))?;
        let room = hub.room_mut(&doc).ok_or("no room")?;
        let editor = ActorId::new("editor")?;
        room.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
        edit(room, "n0")?;
        let refused = commit(room, &editor, 1).map(|_| ()).expect_err("the cabinet refused the version");
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert!(room.is_dirty());
        assert!(room.conflict().is_some(), "/health has to say a session cannot be saved");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn resuming_with_a_different_etag_is_a_conflict() -> Result<(), Box<dyn std::error::Error>> {
        let root = root("resume");
        let config = config(&root, None);
        let doc = DocKey::new("file-1")?;
        let bytes = Arc::new(fixture("one"));
        let seed = Seed { etag: etag_of(&bytes), bytes: bytes.clone(), mirror: None };
        let mut hub = Hub::new(RoomConfig::default());
        open_room(&mut hub, &config, doc.clone(), Some(seed))?;
        drop(hub);
        let other = Arc::new(fixture("two"));
        let changed = Seed { etag: etag_of(&other), bytes: other, mirror: None };
        let mut hub = Hub::new(RoomConfig::default());
        let refused = open_room(&mut hub, &config, doc, Some(changed)).expect_err("the etag differs");
        match refused {
            RoomError::Conflict { version, etag, .. } => {
                assert_eq!(version, 0, "the room is still at the seeded version");
                assert!(etag.is_some());
            }
            other => return Err(format!("unexpected error: {other:?}").into()),
        }
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
