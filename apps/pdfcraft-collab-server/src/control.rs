//! The cabinet control plane: `POST /v1/sessions` and
//! `POST /v1/sessions/{fileId}/resolve` (RFC §8.2, §9).
//!
//! The cabinet calls the first before a user opens the editor. It names the
//! file, presents a session JWT, and supplies the file either inline (base64) or
//! as a URL the daemon fetches. The answer is the room's version and etag; the
//! client then connects over the WebSocket with the same token.
//!
//! Two refusals matter:
//! - `409` when the room already holds a different etag: the cabinet's copy
//!   changed outside the session, or a previous commit was not published. The
//!   session is not started (or resumed) over a file the cabinet moved on from.
//! - `403` when the token names another file, and `401` when it does not verify.
//!
//! The second is the operator's way out of a room a commit callback refused
//! with `409`: it names the version and etag the cabinet holds now, and the room
//! publishes again on the next commit. A room that is not in conflict is
//! refused, so a repeated call cannot move the marker twice.
//!
//! The listener is a plain HTTP/1.1 socket with a body limit and no keep-alive.
//! It is off unless `--control` is given; directory provisioning stays as the
//! dev mode.

use crate::http::{self, Request};
use crate::rooms::{self, RoomError, RoomsConfig, Seed};
use pdfcraft_collab_server::{Adapter, Published};
use pdfcraft_collab_ws::{ResolveError, TransportError, Verifier, WsServer};
use serde::Deserialize;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;

use crate::rooms::CabinetAdapter;

/// The largest request body the control plane accepts. A PDF arrives inline in
/// base64, so this is a little above the source limit.
const MAX_BODY: usize = 384 << 20;

pub struct ControlConfig {
    pub rooms: RoomsConfig,
}

pub struct Control {
    server: Arc<WsServer<CabinetAdapter>>,
    verifier: Arc<dyn Verifier>,
    config: ControlConfig,
}

#[derive(Debug, Deserialize)]
struct SessionRequest {
    #[serde(rename = "fileId")]
    file_id: String,
    jwt: String,
    /// The etag of the copy the cabinet holds. Optional for old callers; when
    /// `bytes` are sent it must be the etag of those bytes.
    #[serde(default)]
    etag: Option<String>,
    /// The file inline, base64-encoded. `initialBytes` is the name the shared
    /// cabinet contract for the five editors uses.
    #[serde(default, alias = "initialBytes")]
    bytes: Option<String>,
    /// A plain-`http` URL the daemon fetches for the file.
    #[serde(default)]
    url: Option<String>,
}

/// What the operator says the cabinet holds now, for a room whose commit was
/// refused with `409`.
#[derive(Debug, Deserialize)]
struct ResolveRequest {
    /// The cabinet's version of its copy. It must be newer than the version
    /// this session already knows, or the next commit would fail the same way.
    #[serde(rename = "expectedVersion")]
    expected_version: u64,
    /// The etag of the cabinet's copy: `sha256:<hex>`.
    etag: String,
}

struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    fn json(status: u16, body: serde_json::Value) -> Self {
        Self { status, body: body.to_string() }
    }

    fn error(status: u16, code: &str, message: impl Into<String>) -> Self {
        Self::json(status, serde_json::json!({ "code": code, "message": message.into() }))
    }
}

impl Control {
    pub fn new(server: Arc<WsServer<CabinetAdapter>>, verifier: Arc<dyn Verifier>, config: ControlConfig) -> Self {
        Self { server, verifier, config }
    }

    /// Listen on `addr` on its own thread. The returned address is the one the
    /// cabinet should call.
    pub fn bind(self: &Arc<Self>, addr: &str) -> Result<SocketAddr, String> {
        let listener = TcpListener::bind(addr).map_err(|e| format!("could not listen for the control plane on {addr}: {e}"))?;
        let bound = listener.local_addr().map_err(|e| format!("control listener: {e}"))?;
        let control = self.clone();
        std::thread::Builder::new()
            .name("pdfcraft-collab-control".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let _ = control.answer(stream);
                }
            })
            .map_err(|e| format!("could not start the control listener: {e}"))?;
        Ok(bound)
    }

    fn answer(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let reply = match http::read_request(&mut stream, MAX_BODY) {
            Ok(request) => self.dispatch(&request),
            Err(e) => Reply::error(400, "invalid", e.0),
        };
        http::respond(&mut stream, &status_line(reply.status), "application/json", reply.body.as_bytes())
    }

    fn dispatch(&self, request: &Request) -> Reply {
        let path = request.path.split('?').next().unwrap_or("/");
        if let Some(file_id) = path.strip_prefix("/v1/sessions/").and_then(|rest| rest.strip_suffix("/resolve")) {
            return match request.method.as_str() {
                "POST" if !file_id.is_empty() && !file_id.contains('/') => self.resolve(file_id, request),
                _ => Reply::error(404, "not_found", "no such control endpoint"),
            };
        }
        match (request.method.as_str(), path) {
            ("POST", "/v1/sessions") => self.session(&request.body),
            ("GET", "/health") => Reply::json(200, serde_json::json!({ "ok": true })),
            _ => Reply::error(404, "not_found", "no such control endpoint"),
        }
    }

    /// `POST /v1/sessions/{fileId}/resolve`: the operator's way out of a
    /// `conflict` room. The token is the cabinet's service token (the same JWT
    /// the session uses, `Authorization: Bearer`), it must be for this file,
    /// and a role that may write (a commenter or viewer session cannot save the
    /// file, so it cannot resolve it either).
    ///
    /// The reply is the room's state after the marker moved. Every refusal
    /// leaves the room untouched.
    fn resolve(&self, file_id: &str, request: &Request) -> Reply {
        let token = match request.header("authorization").and_then(|value| value.strip_prefix("Bearer ")) {
            Some(token) if !token.is_empty() => token,
            _ => return Reply::error(401, "auth", "the request needs a service token: Authorization: Bearer <jwt>"),
        };
        let claims = match self.verifier.verify(token) {
            Ok(claims) => claims,
            Err(e) => return Reply::error(401, "auth", e.to_string()),
        };
        if claims.file_id != file_id {
            return Reply::error(403, "forbidden", "the service token is for another file");
        }
        if !claims.role.can_structural() {
            return Reply::error(403, "forbidden", "this role cannot resolve a conflict");
        }
        let body: ResolveRequest = match serde_json::from_slice(&request.body) {
            Ok(body) => body,
            Err(e) => return Reply::error(400, "invalid", format!("the body is not a resolve request: {e}")),
        };
        let etag = match canonical_etag(&body.etag) {
            Some(etag) => etag,
            None => return Reply::error(400, "invalid", "the etag must be sha256:<64 hex digits>"),
        };
        let doc = match rooms::doc_key(file_id) {
            Ok(doc) => doc,
            Err(e) => return Reply::error(400, "invalid", e),
        };
        let published = Published { version: body.expected_version, etag: etag.clone() };
        match self.server.resolve_conflict(&doc, published) {
            Ok((version, epoch)) => Reply::json(
                200,
                serde_json::json!({
                    "fileId": file_id,
                    "version": version,
                    "epoch": epoch,
                    "etag": etag,
                    "cabinetVersion": body.expected_version,
                    "conflict": false,
                }),
            ),
            Err(ResolveError::Missing) => Reply::error(404, "missing", "no session for this document"),
            Err(ResolveError::NotInConflict) => Reply::error(409, "not_conflict", "the session is not in conflict"),
            Err(ResolveError::Busy) => Reply::error(409, "busy", "a publication of this document is in flight; try again"),
            Err(ResolveError::Invalid(message)) => Reply::error(400, "invalid", message),
            Err(ResolveError::Failed(message)) => Reply::error(500, "internal", message),
            Err(ResolveError::Hub) => Reply::error(503, "unavailable", "the hub is unavailable"),
        }
    }

    fn session(&self, body: &[u8]) -> Reply {
        let request: SessionRequest = match serde_json::from_slice(body) {
            Ok(request) => request,
            Err(e) => return Reply::error(400, "invalid", format!("the body is not a session request: {e}")),
        };
        let claims = match self.verifier.verify(&request.jwt) {
            Ok(claims) => claims,
            Err(e) => return Reply::error(401, "auth", e.to_string()),
        };
        if claims.file_id != request.file_id {
            return Reply::error(403, "forbidden", "the session token is for another file");
        }
        let doc = match rooms::doc_key(&request.file_id) {
            Ok(doc) => doc,
            Err(e) => return Reply::error(400, "invalid", e),
        };
        let seed = match self.source(&request) {
            Ok(seed) => seed,
            Err(reply) => return reply,
        };
        let etag = match (&request.etag, &seed) {
            (Some(etag), Some(seed)) if *etag != seed.etag => {
                return Reply::error(400, "invalid", "the etag does not match the bytes that were sent");
            }
            (Some(etag), _) => Some(etag.clone()),
            (None, Some(seed)) => Some(seed.etag.clone()),
            (None, None) => None,
        };
        let seed = seed.map(|seed| Seed { etag: etag.clone().unwrap_or(seed.etag), ..seed });

        let opened = self.server.with_hub(|hub| {
            if let Some(room) = hub.room(&doc) {
                // The room is live: never open it twice, and check the etag the
                // cabinet sent against the copy the cabinet last accepted.
                if let (Some(incoming), Some(published)) = (&etag, Adapter::published(room.adapter()))
                    && &published.etag != incoming
                {
                    return Err(RoomError::Conflict {
                        message: "the file changed outside the session".into(),
                        etag: Some(published.etag.clone()),
                        version: room.version(),
                    });
                }
                let etag = Adapter::published(room.adapter()).map(|published| published.etag);
                return Ok(rooms::Opened { version: room.version(), epoch: room.epoch(), created: false, etag });
            }
            rooms::open_room(hub, &self.config.rooms, doc, seed)
        });
        let opened = match opened {
            Ok(Ok(opened)) => opened,
            Ok(Err(error)) => return failure(error),
            Err(TransportError::Hub) => return Reply::error(503, "unavailable", "the hub is unavailable"),
            Err(e) => return Reply::error(500, "internal", e.to_string()),
        };
        Reply::json(
            if opened.created { 201 } else { 200 },
            serde_json::json!({
                "fileId": request.file_id,
                "version": opened.version,
                "epoch": opened.epoch,
                "etag": opened.etag,
                "created": opened.created,
            }),
        )
    }

    /// Read the file the cabinet named: inline bytes or a URL. No source means
    /// "resume only".
    fn source(&self, request: &SessionRequest) -> Result<Option<Seed>, Reply> {
        match (&request.bytes, &request.url) {
            (Some(_), Some(_)) => Err(Reply::error(400, "invalid", "send either bytes or a url, not both")),
            (Some(text), None) => {
                let bytes = rooms::base64_decode(text).map_err(|e| Reply::error(400, "invalid", format!("the bytes are not base64: {e}")))?;
                self.seed(bytes)
            }
            (None, Some(url)) => {
                let client = http::Client { timeout: self.config.rooms.http_timeout };
                let (status, bytes) = client.get(url, self.config.rooms.max_source_bytes).map_err(|e| Reply::error(502, "source", e.0))?;
                if !(200..300).contains(&status) {
                    return Err(Reply::error(502, "source", format!("the source answered {status}")));
                }
                self.seed(bytes)
            }
            (None, None) => Ok(None),
        }
    }

    fn seed(&self, bytes: Vec<u8>) -> Result<Option<Seed>, Reply> {
        if bytes.len() > self.config.rooms.max_source_bytes {
            return Err(Reply::error(413, "limit", format!("the file is larger than {} bytes", self.config.rooms.max_source_bytes)));
        }
        if !bytes.starts_with(b"%PDF-") {
            return Err(Reply::error(400, "invalid", "the source is not a PDF"));
        }
        let etag = rooms::etag_of(&bytes);
        Ok(Some(Seed { bytes: Arc::new(bytes), etag, mirror: None }))
    }
}

/// The canonical `sha256:<lowercase hex>` form of an etag, or `None` when the
/// string is not one. A cabinet that sends upper-case hex is normalised, so the
/// marker compares equal to the daemon's own etags.
fn canonical_etag(etag: &str) -> Option<String> {
    let hex = etag.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("sha256:{}", hex.to_ascii_lowercase()))
}

fn failure(error: RoomError) -> Reply {
    match error {
        RoomError::Conflict { message, etag, version } => {
            Reply::json(409, serde_json::json!({ "code": "conflict", "message": message, "etag": etag, "version": version }))
        }
        RoomError::Missing(message) => Reply::error(404, "missing", message),
        RoomError::Invalid(message) => Reply::error(400, "invalid", message),
    }
}

fn status_line(status: u16) -> String {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    format!("{status} {reason}")
}

#[cfg(test)]
mod tests {
    use super::{Control, ControlConfig};
    use crate::http::Client;
    use crate::rooms::{self, Publisher, RoomsConfig};
    use pdfcraft_collab_hub::Hub;
    use pdfcraft_collab_proto::Role;
    use pdfcraft_collab_server::RoomConfig;
    use pdfcraft_collab_ws::{Claims, Config as WsConfig, TokenError, Verifier, WsServer};
    use std::sync::Arc;
    use std::time::Duration;

    fn fixture(label: &str) -> Vec<u8> {
        let body = format!("BT /F1 18 Tf 20 150 Td ({label}) Tj ET");
        let objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes(),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        out
    }

    struct TestVerifier;

    impl Verifier for TestVerifier {
        fn verify(&self, token: &str) -> Result<Claims, TokenError> {
            match token {
                "good" => Ok(Claims {
                    subject: "user-7".into(),
                    audience: "lzmkm".into(),
                    file_id: "file-1".into(),
                    app: "pdfcraft".into(),
                    role: Role::Editor,
                    session_id: "session-9".into(),
                    expires_at: u64::MAX,
                }),
                "good-2" => Ok(Claims {
                    subject: "user-7".into(),
                    audience: "lzmkm".into(),
                    file_id: "file-2".into(),
                    app: "pdfcraft".into(),
                    role: Role::Editor,
                    session_id: "session-9".into(),
                    expires_at: u64::MAX,
                }),
                "good-9" => Ok(Claims {
                    subject: "user-7".into(),
                    audience: "lzmkm".into(),
                    file_id: "file-9".into(),
                    app: "pdfcraft".into(),
                    role: Role::Editor,
                    session_id: "session-9".into(),
                    expires_at: u64::MAX,
                }),
                "other-file" => Ok(Claims {
                    subject: "user-7".into(),
                    audience: "lzmkm".into(),
                    file_id: "file-2".into(),
                    app: "pdfcraft".into(),
                    role: Role::Editor,
                    session_id: "session-9".into(),
                    expires_at: u64::MAX,
                }),
                "viewer" => Ok(Claims {
                    subject: "user-9".into(),
                    audience: "lzmkm".into(),
                    file_id: "file-1".into(),
                    app: "pdfcraft".into(),
                    role: Role::Viewer,
                    session_id: "session-9".into(),
                    expires_at: u64::MAX,
                }),
                _ => Err(TokenError::Invalid),
            }
        }
    }

    fn root(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("pdfcraft-collab-control-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn control(name: &str, publisher: Option<Arc<Publisher>>) -> (Arc<Control>, std::path::PathBuf) {
        let root = root(name);
        let rooms = RoomsConfig { data: root.join("data"), publisher, max_source_bytes: 8 << 20, http_timeout: Duration::from_secs(5) };
        let hub = Hub::new(RoomConfig::default());
        let server = Arc::new(WsServer::new(hub, Arc::new(TestVerifier), WsConfig::default()));
        let control = Arc::new(Control::new(server, Arc::new(TestVerifier), ControlConfig { rooms }));
        (control, root)
    }

    fn post(address: std::net::SocketAddr, body: serde_json::Value) -> Result<(u16, serde_json::Value), String> {
        post_to(address, "/v1/sessions", Some(body.get("jwt").and_then(|jwt| jwt.as_str()).unwrap_or("good")), &body)
    }

    fn post_to(address: std::net::SocketAddr, path: &str, token: Option<&str>, body: &serde_json::Value) -> Result<(u16, serde_json::Value), String> {
        let client = Client { timeout: Duration::from_secs(5) };
        let encoded = body.to_string();
        let (status, reply) = client.post_json(&format!("http://{address}{path}"), token, &[], encoded.as_bytes()).map_err(|e| e.0)?;
        Ok((status, serde_json::from_slice(&reply).map_err(|e| e.to_string())?))
    }

    /// A cabinet that answers one status to every commit callback.
    fn always_cabinet(status: u16) -> std::io::Result<String> {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
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
                let mut stream = match stream.try_clone() {
                    Ok(clone) => clone,
                    Err(_) => return,
                };
                let answer = "{\"code\":\"conflict\"}";
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                let _ = stream.flush();
            }
        });
        Ok(format!("http://{address}/commit"))
    }

    #[test]
    fn a_session_is_created_from_inline_bytes_and_found_again() -> Result<(), Box<dyn std::error::Error>> {
        let (control, root) = control("create", None);
        let address = control.bind("127.0.0.1:0")?;
        let pdf = fixture("one");
        let body = serde_json::json!({"fileId": "file-1", "jwt": "good", "bytes": rooms::base64_encode(&pdf)});
        let (status, reply) = post(address, body.clone())?;
        assert_eq!(status, 201, "{reply}");
        assert_eq!(reply["fileId"], "file-1");
        assert_eq!(reply["version"], 0);
        assert_eq!(reply["etag"], rooms::etag_of(&pdf));

        // The same session again: resumed, not created, and the room is live.
        let (status, again) = post(address, body)?;
        assert_eq!(status, 200, "{again}");
        assert_eq!(again["created"], false);

        // The shared cabinet contract names the file `initialBytes`; both spellings
        // open a room.
        let shared = serde_json::json!({"fileId": "file-2", "jwt": "good-2", "initialBytes": rooms::base64_encode(&pdf)});
        let (status, reply) = post(address, shared)?;
        assert_eq!(status, 201, "{reply}");
        assert_eq!(reply["etag"], rooms::etag_of(&pdf));
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn a_changed_copy_is_refused_with_409() -> Result<(), Box<dyn std::error::Error>> {
        let (control, root) = control("conflict", None);
        let address = control.bind("127.0.0.1:0")?;
        let first = fixture("one");
        let (status, _) = post(address, serde_json::json!({"fileId": "file-1", "jwt": "good", "bytes": rooms::base64_encode(&first)}))?;
        assert_eq!(status, 201);

        // The cabinet replaced the file under the live room.
        let second = fixture("two");
        let (status, reply) = post(
            address,
            serde_json::json!({"fileId": "file-1", "jwt": "good", "etag": rooms::etag_of(&second), "bytes": rooms::base64_encode(&second)}),
        )?;
        assert_eq!(status, 409, "{reply}");
        assert_eq!(reply["code"], "conflict");
        assert_eq!(reply["etag"], rooms::etag_of(&first));
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn the_token_decides_the_file() -> Result<(), Box<dyn std::error::Error>> {
        let (control, root) = control("auth", None);
        let address = control.bind("127.0.0.1:0")?;
        let pdf = fixture("one");
        let body = serde_json::json!({"fileId": "file-1", "jwt": "bad", "bytes": rooms::base64_encode(&pdf)});
        let (status, reply) = post(address, body)?;
        assert_eq!(status, 401, "{reply}");
        let body = serde_json::json!({"fileId": "file-1", "jwt": "other-file", "bytes": rooms::base64_encode(&pdf)});
        let (status, reply) = post(address, body)?;
        assert_eq!(status, 403, "{reply}");
        let body = serde_json::json!({"fileId": "file-1", "jwt": "good"});
        let (status, reply) = post(address, body)?;
        assert_eq!(status, 404, "no source and no room: {reply}");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn an_etag_that_does_not_match_the_bytes_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let (control, root) = control("etag", None);
        let address = control.bind("127.0.0.1:0")?;
        let pdf = fixture("one");
        let body = serde_json::json!({
            "fileId": "file-1", "jwt": "good", "etag": "sha256:00", "bytes": rooms::base64_encode(&pdf)
        });
        let (status, reply) = post(address, body)?;
        assert_eq!(status, 400, "{reply}");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn a_source_url_is_fetched_and_a_wrong_content_type_is_not_accepted() -> Result<(), Box<dyn std::error::Error>> {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}/file.pdf", listener.local_addr()?);
        let pdf = fixture("served");
        let served = pdf.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut head = [0u8; 2048];
                let _ = stream.read(&mut head);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/pdf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    served.len()
                );
                let _ = stream.write_all(&served);
            }
        });
        let (control, root) = control("url", None);
        let address = control.bind("127.0.0.1:0")?;
        let (status, reply) = post(address, serde_json::json!({"fileId": "file-1", "jwt": "good", "url": url}))?;
        assert_eq!(status, 201, "{reply}");
        assert_eq!(reply["etag"], rooms::etag_of(&pdf));

        // A source that is not a PDF is refused before anything is opened.
        let (status, reply) = post(address, serde_json::json!({"fileId": "file-1", "jwt": "good", "url": "http://127.0.0.1:1/file.pdf"}))?;
        assert_eq!(status, 502, "{reply}");
        let (status, reply) = post(address, serde_json::json!({"fileId": "file-1", "jwt": "good", "bytes": rooms::base64_encode(b"not a pdf")}))?;
        assert_eq!(status, 400, "{reply}");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn a_room_resumed_from_disk_keeps_its_version() -> Result<(), Box<dyn std::error::Error>> {
        // First process: create the room and leave one unpublished operation.
        let root = root("resume");
        let rooms = RoomsConfig { data: root.join("data"), publisher: None, max_source_bytes: 8 << 20, http_timeout: Duration::from_secs(5) };
        let pdf = fixture("one");
        let seed = rooms::Seed { etag: rooms::etag_of(&pdf), bytes: Arc::new(pdf.clone()), mirror: None };
        let doc = pdfcraft_collab_proto::DocKey::new("file-1")?;
        {
            let mut hub = Hub::new(RoomConfig::default());
            rooms::open_room(&mut hub, &rooms, doc.clone(), Some(seed))?;
            let room = hub.room_mut(&doc).ok_or("no room")?;
            let actor = pdfcraft_collab_proto::ActorId::new("editor")?;
            room.hello(
                actor.clone(),
                Role::Editor,
                &pdfcraft_collab_proto::Hello {
                    v: 1,
                    app: "pdfcraft".into(),
                    doc: doc.clone(),
                    jwt: "t".into(),
                    client: pdfcraft_collab_proto::ClientInfo { name: "test".into(), version: "0".into() },
                },
                0,
            )?;
            let op = pdfcraft_collab_proto::Op {
                op_id: pdfcraft_collab_proto::OpId::new("n0")?,
                doc: doc.clone(),
                base_version: 0,
                epoch: 0,
                kind: pdfcraft_collab_proto::OpKind::Primitive,
                class: pdfcraft_collab_proto::ConflictClass::Merge,
                target: pdfcraft_collab_proto::Target::Page(pdfcraft_collab_proto::PageRef { index: 0, epoch: 0 }),
                payload: serde_json::json!({"action": "annotation.add", "at": [10.0, 10.0], "contents": "x", "nm": "n0"}),
                preview: None,
            };
            room.apply(&actor, &op, 0)?;
        }
        // Second process: the control plane resumes from the journal.
        let hub = Hub::new(RoomConfig::default());
        let server = Arc::new(WsServer::new(hub, Arc::new(TestVerifier), WsConfig::default()));
        let control = Arc::new(Control::new(server, Arc::new(TestVerifier), ControlConfig { rooms }));
        let address = control.bind("127.0.0.1:0")?;
        let (status, reply) = post(address, serde_json::json!({"fileId": "file-1", "jwt": "good", "etag": rooms::etag_of(&pdf)}))?;
        assert_eq!(status, 200, "{reply}");
        assert_eq!(reply["version"], 1, "the journal tail was replayed");
        assert_eq!(reply["created"], false);
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// Put the room in `conflict` the way production does: a commit whose
    /// cabinet callback answers `409`.
    fn make_conflict(control: &Control, doc: &pdfcraft_collab_proto::DocKey) -> Result<(), String> {
        use pdfcraft_collab_proto::{ActorId, ClientInfo, ConflictClass, Hello, Op, OpId, OpKind, PageRef, Role as ProtoRole, Target};
        control
            .server
            .with_hub(|hub| -> Result<(), String> {
                let room = hub.room_mut(doc).ok_or_else(|| "no room".to_owned())?;
                let editor = ActorId::new("user-7").map_err(|e| e.to_string())?;
                let hello = Hello {
                    v: 1,
                    app: "pdfcraft".into(),
                    doc: doc.clone(),
                    jwt: "good".into(),
                    client: ClientInfo { name: "test".into(), version: "0".into() },
                };
                room.hello(editor.clone(), ProtoRole::Editor, &hello, 0).map_err(|e| e.message)?;
                let op = Op {
                    op_id: OpId::new("conflict-1").map_err(|e| e.to_string())?,
                    doc: doc.clone(),
                    base_version: 0,
                    epoch: 0,
                    kind: OpKind::Primitive,
                    class: ConflictClass::Merge,
                    target: Target::Page(PageRef { index: 0, epoch: 0 }),
                    payload: serde_json::json!({"action": "annotation.add", "at": [10.0, 10.0], "contents": "x", "nm": "conflict-1"}),
                    preview: None,
                };
                room.apply(&editor, &op, 0).map_err(|e| e.message)?;
                let commit = room.commit(&editor, 1).map_err(|e| e.message)?;
                let outcome = commit.publish();
                let _ = room.finish(commit, outcome);
                Ok(())
            })
            .map_err(|e| e.to_string())?
    }

    #[test]
    fn an_operator_resolves_a_conflict_and_a_second_call_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let root = root("resolve");
        let url = always_cabinet(409)?;
        let publisher = Arc::new(Publisher::new(url, None, Duration::from_secs(5)));
        let rooms =
            RoomsConfig { data: root.join("data"), publisher: Some(publisher), max_source_bytes: 8 << 20, http_timeout: Duration::from_secs(5) };
        let hub = Hub::new(RoomConfig::default());
        let server = Arc::new(WsServer::new(hub, Arc::new(TestVerifier), WsConfig::default()));
        let control = Arc::new(Control::new(server, Arc::new(TestVerifier), ControlConfig { rooms }));
        let address = control.bind("127.0.0.1:0")?;
        let pdf = fixture("one");
        let body = serde_json::json!({"fileId": "file-1", "jwt": "good", "bytes": rooms::base64_encode(&pdf)});
        assert_eq!(post(address, body)?.0, 201);
        let doc = pdfcraft_collab_proto::DocKey::new("file-1")?;
        make_conflict(&control, &doc)?;
        let metrics = control.server.metrics()?;
        assert_eq!(metrics.conflict, vec!["file-1".to_owned()], "the room is flagged");

        let path = "/v1/sessions/file-1/resolve";
        let etag = |byte: &str| format!("sha256:{}", byte.repeat(32));
        let (fresh, next, zero) = (etag("44"), etag("55"), etag("00"));
        let resolve = |version: u64, etag: &str| serde_json::json!({"expectedVersion": version, "etag": etag});

        // No token, another file's token, a viewer's token: refused.
        let (status, reply) = post_to(address, path, None, &resolve(4, &fresh))?;
        assert_eq!(status, 401, "{reply}");
        let (status, reply) = post_to(address, path, Some("other-file"), &resolve(4, &fresh))?;
        assert_eq!(status, 403, "{reply}");
        let (status, reply) = post_to(address, path, Some("viewer"), &resolve(4, &fresh))?;
        assert_eq!(status, 403, "{reply}");
        // Malformed etag, and a version the session already holds: refused.
        let (status, reply) = post_to(address, path, Some("good"), &resolve(4, "not-an-etag"))?;
        assert_eq!(status, 400, "{reply}");
        let (status, reply) = post_to(address, path, Some("good"), &resolve(0, &zero))?;
        assert_eq!(status, 400, "{reply}");
        assert_eq!(control.server.metrics()?.conflict, vec!["file-1".to_owned()], "a refusal changes nothing");
        // A document with no room: 404.
        let (status, reply) = post_to(address, "/v1/sessions/file-9/resolve", Some("good-9"), &resolve(4, &fresh))?;
        assert_eq!(status, 404, "{reply}");

        // The operator's answer: the cabinet is at version 4.
        let (status, reply) = post_to(address, path, Some("good"), &resolve(4, &fresh))?;
        assert_eq!(status, 200, "{reply}");
        assert_eq!(reply["fileId"], "file-1");
        assert_eq!(reply["version"], 1, "the room's own version is unchanged");
        assert_eq!(reply["cabinetVersion"], 4);
        assert_eq!(reply["etag"], fresh);
        assert_eq!(reply["conflict"], false);
        let metrics = control.server.metrics()?;
        assert!(metrics.conflict.is_empty(), "the room is not flagged any more");
        assert_eq!(metrics.conflicts_resolved, 1);
        assert_eq!(control.server.with_hub(|hub| hub.room(&doc).map(|room| room.conflict().is_none()))?, Some(true));

        // A second resolve has nothing to resolve and is refused.
        let (status, reply) = post_to(address, path, Some("good"), &resolve(5, &next))?;
        assert_eq!(status, 409, "{reply}");
        assert_eq!(reply["code"], "not_conflict");
        assert_eq!(control.server.metrics()?.conflicts_resolved, 1, "a refusal is not counted");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn a_live_room_reports_its_etag_and_a_replaced_file_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let (control, root) = control("live", None);
        let address = control.bind("127.0.0.1:0")?;
        let pdf = fixture("one");
        let body = serde_json::json!({"fileId": "file-1", "jwt": "good", "bytes": rooms::base64_encode(&pdf)});
        assert_eq!(post(address, body)?.0, 201);
        let replacement = fixture("two");
        let body = serde_json::json!({"fileId": "file-1", "jwt": "good", "bytes": rooms::base64_encode(&replacement)});
        let (status, reply) = post(address, body)?;
        assert_eq!(status, 409, "{reply}");
        assert_eq!(reply["etag"], rooms::etag_of(&pdf));
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
