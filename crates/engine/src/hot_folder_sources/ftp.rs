//! Bounded passive FTP subset (RFC 959/2428/3659) and explicit TLS (RFC 4217).
//! Never uses active mode, follows server-supplied data hosts, or downgrades TLS.

use super::*;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(10);
static RESOLVERS: AtomicUsize = AtomicUsize::new(0);

pub(super) fn validate(source: &FtpSource) -> Result<(), String> {
    if source.host.is_empty() || source.host.len() > 253 || source.host.contains(['/', '\\', '\r', '\n', '\0', '@', ' ']) || source.port == 0 {
        return Err("FTP needs a hostname without URL/user information and a port from 1 to 65535".into());
    }
    if source.username.is_empty() || source.username.len() > 256 || source.username.contains(['\r', '\n', '\0']) {
        return Err("FTP username must be 1..256 bytes without line breaks".into());
    }
    credentials::valid_id(&source.credential_id)?;
    remote_path(&source.folder)?;
    if let SourceAction::Move { destination } = &source.action {
        remote_path(destination)?;
        if destination == &source.folder || destination.starts_with(&format!("{}/", source.folder.trim_end_matches('/'))) {
            return Err("FTP archive folder must be outside the watched folder".into());
        }
    }
    Ok(())
}

fn remote_path(value: &str) -> Result<(), String> {
    if !value.starts_with('/')
        || value.len() > 4096
        || value.contains(['\r', '\n', '\0', '\\'])
        || value.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err("FTP folder must be an absolute slash path without traversal, backslashes or line breaks".into());
    }
    Ok(())
}

enum Socket {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, BoundedTcp>>),
}

/// rustls can perform several socket reads inside one public read/handshake. Keep
/// the whole-operation deadline on the underlying I/O too, including slow peers.
struct BoundedTcp {
    socket: TcpStream,
    deadline: Instant,
}
impl BoundedTcp {
    fn timeout(&self) -> std::io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .map(|duration| duration.min(SOCKET_TIMEOUT))
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "FTPS operation deadline exceeded"))
    }
}
impl Read for BoundedTcp {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.socket.set_read_timeout(Some(self.timeout()?))?;
        self.socket.read(bytes)
    }
}
impl Write for BoundedTcp {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.socket.set_write_timeout(Some(self.timeout()?))?;
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.socket.flush()
    }
}
impl Read for Socket {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(socket) => socket.read(bytes),
            Self::Tls(socket) => socket.read(bytes),
        }
    }
}
impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(socket) => socket.write(bytes),
            Self::Tls(socket) => socket.write(bytes),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(socket) => socket.flush(),
            Self::Tls(socket) => socket.flush(),
        }
    }
}

pub(super) struct Client {
    source: FtpSource,
    socket: Socket,
    peer: SocketAddr,
    tls: Option<Arc<ClientConfig>>,
}

impl Client {
    pub(super) fn connect(store: &CredentialStore, source: &FtpSource, control: &Control<'_>) -> Result<Self, String> {
        validate(source)?;
        let password = credentials::ftp_password(store, &source.credential_id)?;
        Self::connect_password(source, &password, control)
    }
    fn connect_password(source: &FtpSource, password: &str, control: &Control<'_>) -> Result<Self, String> {
        control.check()?;
        let addresses = resolve(&source.host, source.port, control)?;
        let mut connected = None;
        for address in addresses.into_iter().take(8) {
            control.check()?;
            if let Ok(socket) = TcpStream::connect_timeout(&address, SOCKET_TIMEOUT) {
                connected = Some(socket);
                break;
            }
        }
        let socket = connected.ok_or("FTP connection failed or timed out")?;
        socket.set_read_timeout(Some(SOCKET_TIMEOUT)).and_then(|_| socket.set_write_timeout(Some(SOCKET_TIMEOUT))).map_err(io_error)?;
        let peer = socket.peer_addr().map_err(io_error)?;
        let mut client = Self { source: source.clone(), socket: Socket::Plain(socket), peer, tls: None };
        let welcome = client.reply(control)?.0;
        if welcome != 220 {
            return Err("FTP server did not provide a ready greeting".into());
        }
        if source.security == FtpSecurity::ExplicitTls {
            client.expect("AUTH TLS", &[234], control)?;
            let mut roots = RootCertStore::empty();
            let certificates = rustls_native_certs::load_native_certs();
            for certificate in certificates.certs {
                let _ = roots.add(certificate);
            }
            if roots.is_empty() {
                return Err("no trusted OS certificate authorities are available for FTPS".into());
            }
            let tls = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|_| "cannot configure FTPS protocol versions")?
                .with_root_certificates(roots)
                .with_no_client_auth();
            let tls = Arc::new(tls);
            let Socket::Plain(socket) = client.socket else {
                return Err("invalid FTP TLS state".into());
            };
            client.socket = secure(socket, &source.host, tls.clone(), control)?;
            client.tls = Some(tls);
            client.expect("PBSZ 0", &[200], control)?;
            client.expect("PROT P", &[200], control)?;
        }
        let user = client.command(&format!("USER {}", source.username), control)?.0;
        if user == 331 {
            client.expect(&format!("PASS {password}"), &[230], control)?;
        } else if user != 230 {
            return Err("FTP login rejected; check the stored credentials".into());
        }
        client.expect("TYPE I", &[200], control)?;
        // UTF-8 negotiation is optional; listings themselves must decode as UTF-8.
        let _ = client.command("OPTS UTF8 ON", control)?;
        Ok(client)
    }

    fn command(&mut self, command: &str, control: &Control<'_>) -> Result<(u16, String), String> {
        control.check()?;
        if command.contains(['\r', '\n', '\0']) || command.len() > 8192 {
            return Err("FTP command contains an invalid path or credential".into());
        }
        self.socket
            .write_all(command.as_bytes())
            .and_then(|_| self.socket.write_all(b"\r\n"))
            .and_then(|_| self.socket.flush())
            .map_err(|_| "FTP command failed or timed out".to_owned())?;
        self.reply(control)
    }
    fn expect(&mut self, command: &str, expected: &[u16], control: &Control<'_>) -> Result<String, String> {
        let (status, text) = self.command(command, control)?;
        if !expected.contains(&status) {
            return Err(format!("FTP server rejected the operation (status {status}); original retained"));
        }
        Ok(text)
    }
    fn reply(&mut self, control: &Control<'_>) -> Result<(u16, String), String> {
        let mut text = String::new();
        let mut first = None;
        for _ in 0..128 {
            control.check()?;
            let mut line = Vec::new();
            for _ in 0..4096 {
                control.check()?;
                let mut byte = [0_u8];
                self.socket.read_exact(&mut byte).map_err(|_| "FTP response failed or timed out".to_owned())?;
                line.extend_from_slice(&byte);
                if byte.first() == Some(&b'\n') {
                    break;
                }
            }
            if line.last() != Some(&b'\n') || text.len().saturating_add(line.len()) > 64 * 1024 {
                return Err("FTP response exceeds the size limit".into());
            }
            let line = std::str::from_utf8(&line).map_err(|_| "FTP response is not UTF-8")?;
            let code = line.get(..3).and_then(|value| value.parse::<u16>().ok());
            text.push_str(line);
            if first.is_none() {
                let code = code.ok_or("FTP response has no status code")?;
                if line.as_bytes().get(3) == Some(&b' ') {
                    return Ok((code, text));
                }
                if line.as_bytes().get(3) != Some(&b'-') {
                    return Err("invalid FTP multiline response".into());
                }
                first = Some(code);
            } else if code == first && line.as_bytes().get(3) == Some(&b' ') {
                return Ok((code.ok_or("missing FTP response code")?, text));
            }
        }
        Err("FTP response has too many lines".into())
    }
    fn passive(&mut self, control: &Control<'_>) -> Result<TcpStream, String> {
        let (status, response) = self.command("EPSV", control)?;
        let port = if status == 229 {
            epsv_port(&response)?
        } else if matches!(status, 500 | 501 | 502 | 504 | 522) {
            let response = self.expect("PASV", &[227], control)?;
            pasv_port(&response)?
        } else {
            return Err(format!("FTP passive mode rejected (status {status})"));
        };
        let address = SocketAddr::new(self.peer.ip(), port);
        let socket = TcpStream::connect_timeout(&address, SOCKET_TIMEOUT).map_err(|_| "FTP data connection failed or timed out")?;
        socket.set_read_timeout(Some(SOCKET_TIMEOUT)).and_then(|_| socket.set_write_timeout(Some(SOCKET_TIMEOUT))).map_err(io_error)?;
        Ok(socket)
    }
    fn transfer(&mut self, command: &str, writer: &mut dyn Write, limit: u64, control: &Control<'_>) -> Result<u64, String> {
        let socket = self.passive(control)?;
        self.expect(command, &[125, 150], control)?;
        let mut data = if let Some(tls) = &self.tls { secure(socket, &self.source.host, tls.clone(), control)? } else { Socket::Plain(socket) };
        let count = copy_limited(&mut data, writer, limit, control)?;
        drop(data);
        if !matches!(self.reply(control)?.0, 226 | 250) {
            return Err("FTP transfer did not finish successfully".into());
        }
        Ok(count)
    }
    fn listing(&mut self, folder: &str, control: &Control<'_>) -> Result<Vec<Entry>, String> {
        remote_path(folder)?;
        let mut bytes = Vec::new();
        self.transfer(&format!("MLSD {folder}"), &mut bytes, MAX_JSON, control)?;
        parse_listing(&bytes)
    }
    fn metadata(&mut self, path: &str, control: &Control<'_>) -> Result<Option<Entry>, String> {
        remote_path(path)?;
        let (parent, name) = path.rsplit_once('/').ok_or("invalid remote path")?;
        Ok(self.listing(if parent.is_empty() { "/" } else { parent }, control)?.into_iter().find(|entry| entry.name == name && !entry.directory))
    }
    fn verify(&mut self, record: &Record, control: &Control<'_>) -> Result<bool, String> {
        let Some(meta) = self.metadata(&record.remote.key, control)? else {
            return Ok(false);
        };
        if meta.size != record.remote.size || meta.revision != record.remote.revision {
            return Err("FTP original changed after processing; it was not moved or deleted".into());
        }
        let mut sink = HashWriter::default();
        self.transfer(&format!("RETR {}", record.remote.key), &mut sink, record.remote.size, control)?;
        if sink.finish() != record.source.sha256 {
            return Err("FTP original content changed after processing; it was not moved or deleted".into());
        }
        Ok(true)
    }
}

impl Backend for Client {
    fn list(&mut self, control: &Control<'_>) -> Result<Vec<RemoteFile>, String> {
        let mut pending = vec![(self.source.folder.clone(), PathBuf::new(), 0_usize)];
        let mut result = Vec::new();
        let mut count = 0_usize;
        while let Some((folder, relative, depth)) = pending.pop() {
            control.check()?;
            let mut entries = self.listing(&folder, control)?;
            entries.sort_by(|left, right| left.name.cmp(&right.name));
            count = count.saturating_add(entries.len());
            if count > MAX_ITEMS {
                return Err("FTP listing exceeds 10000 entries".into());
            }
            for entry in entries {
                let path = format!("{}/{}", folder.trim_end_matches('/'), entry.name);
                let relative = relative.join(&entry.name);
                if entry.directory && self.source.recursive {
                    if depth >= 16 {
                        return Err("FTP folder nesting exceeds 16 levels".into());
                    }
                    pending.push((path, relative, depth + 1));
                } else if !entry.directory && supported(&relative) {
                    result.push(RemoteFile { key: path, relative, revision: entry.revision, size: entry.size, group: None });
                }
            }
        }
        result.sort_by(|left, right| left.relative.cmp(&right.relative));
        Ok(result)
    }
    fn read(&mut self, file: &RemoteFile, writer: &mut dyn Write, control: &Control<'_>) -> Result<(), String> {
        let meta = self.metadata(&file.key, control)?.ok_or("FTP source disappeared before download")?;
        if meta.size != file.size || meta.revision != file.revision {
            return Err("FTP source changed before download; retry after it settles".into());
        }
        let size = self.transfer(&format!("RETR {}", file.key), writer, file.size, control)?;
        let meta = self.metadata(&file.key, control)?.ok_or("FTP source disappeared during download")?;
        if size != file.size || meta.size != file.size || meta.revision != file.revision {
            return Err("FTP source changed during download; partial download was discarded".into());
        }
        Ok(())
    }
    fn finish(&mut self, files: &[Record], action: &SourceAction, control: &Control<'_>) -> Result<(), String> {
        for record in files {
            control.check()?;
            if !record.remote.key.starts_with(&format!("{}/", self.source.folder.trim_end_matches('/'))) {
                return Err("FTP receipt does not belong to the watched folder".into());
            }
            match action {
                SourceAction::Keep => {}
                SourceAction::Delete => {
                    // Missing source is an idempotent success after an interrupted delete.
                    if self.verify(record, control)? {
                        self.expect(&format!("DELE {}", record.remote.key), &[250], control)?;
                    }
                }
                SourceAction::Move { destination } => {
                    remote_path(destination)?;
                    // Stable opaque prefix avoids clobbering an unrelated archive filename.
                    let target = format!("{}/{}", destination.trim_end_matches('/'), record.source.staged_relative.to_string_lossy());
                    if let Some(existing) = self.metadata(&target, control)? {
                        let mut sink = HashWriter::default();
                        if existing.size != record.remote.size {
                            return Err("FTP archive filename is occupied; original retained".into());
                        }
                        self.transfer(&format!("RETR {target}"), &mut sink, record.remote.size, control)?;
                        if sink.finish() == record.source.sha256 && self.metadata(&record.remote.key, control)?.is_none() {
                            continue;
                        }
                        return Err("FTP archive filename is occupied; original retained".into());
                    }
                    if !self.verify(record, control)? {
                        return Err("FTP original and verified archive are both missing".into());
                    }
                    self.expect(&format!("RNFR {}", record.remote.key), &[350], control)?;
                    self.expect(&format!("RNTO {target}"), &[250], control)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl HashWriter {
    fn finish(self) -> String {
        self.0.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

fn secure(socket: TcpStream, host: &str, config: Arc<ClientConfig>, control: &Control<'_>) -> Result<Socket, String> {
    let name = rustls::pki_types::ServerName::try_from(host.to_owned()).map_err(|_| "invalid FTPS server name")?;
    let mut connection = ClientConnection::new(config, name).map_err(|_| "cannot initialize FTPS")?;
    let mut socket = BoundedTcp { socket, deadline: control.deadline };
    while connection.is_handshaking() {
        control.check()?;
        connection
            .complete_io(&mut socket)
            .map_err(|_| "FTPS TLS handshake failed: check the server name, trusted certificate and network (TLS was not downgraded)")?;
    }
    Ok(Socket::Tls(Box::new(StreamOwned::new(connection, socket))))
}
fn resolve(host: &str, port: u16, control: &Control<'_>) -> Result<Vec<SocketAddr>, String> {
    if RESOLVERS.fetch_add(1, Ordering::SeqCst) >= 4 {
        RESOLVERS.fetch_sub(1, Ordering::SeqCst);
        return Err("too many pending FTP hostname lookups; retry later".into());
    }
    let host = host.to_owned();
    let (sender, receiver) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new().name("pdfcraft-ftp-dns".into()).spawn(move || {
        let result = (host.as_str(), port).to_socket_addrs().map(|addresses| addresses.take(8).collect::<Vec<_>>());
        let _ = sender.send(result);
        RESOLVERS.fetch_sub(1, Ordering::SeqCst);
    });
    if spawned.is_err() {
        RESOLVERS.fetch_sub(1, Ordering::SeqCst);
        return Err("cannot start FTP hostname lookup".into());
    }
    let deadline = Instant::now() + SOCKET_TIMEOUT;
    loop {
        control.check()?;
        if Instant::now() > deadline {
            return Err("FTP hostname lookup timed out".into());
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(addresses)) => return Ok(addresses),
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => return Err("FTP hostname lookup failed".into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}
fn parenthesized(value: &str) -> Result<&str, String> {
    value.rsplit_once('(').and_then(|(_, tail)| tail.split_once(')')).map(|(inner, _)| inner).ok_or_else(|| "invalid FTP passive response".into())
}
fn epsv_port(value: &str) -> Result<u16, String> {
    let value = parenthesized(value)?;
    let delimiter = value.chars().next().ok_or("invalid FTP extended passive response")?;
    let parts = value.split(delimiter).collect::<Vec<_>>();
    if parts.len() != 5 || parts.first() != Some(&"") || parts.get(1) != Some(&"") || parts.get(2) != Some(&"") || parts.get(4) != Some(&"") {
        return Err("invalid FTP extended passive response".into());
    }
    let port = parts.get(3).and_then(|value| value.parse::<u16>().ok()).filter(|port| *port > 0).ok_or("invalid FTP data port")?;
    Ok(port)
}
fn pasv_port(value: &str) -> Result<u16, String> {
    let parts = parenthesized(value)?.split(',').map(str::parse::<u8>).collect::<Result<Vec<_>, _>>().map_err(|_| "invalid FTP passive response")?;
    if parts.len() != 6 {
        return Err("invalid FTP passive response".into());
    }
    let port = u16::from(*parts.get(4).ok_or("invalid FTP passive port")?) * 256 + u16::from(*parts.get(5).ok_or("invalid FTP passive port")?);
    if port == 0 {
        return Err("invalid FTP passive port".into());
    }
    Ok(port)
}
struct Entry {
    name: String,
    directory: bool,
    size: u64,
    revision: String,
}
fn parse_listing(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "FTP listing is not UTF-8; this source requires RFC 3659 MLSD")?;
    let mut result = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        if result.len() >= MAX_ITEMS || line.len() > 8192 {
            return Err("FTP listing exceeds its entry/line limit".into());
        }
        let (facts, name) = line.split_once(' ').ok_or("FTP server must support RFC 3659 MLSD listings")?;
        let facts = facts
            .split(';')
            .filter_map(|fact| fact.split_once('='))
            .map(|(key, value)| (key.to_ascii_lowercase(), value))
            .collect::<BTreeMap<_, _>>();
        let kind = facts.get("type").copied().unwrap_or_default();
        if matches!(kind, "cdir" | "pdir") || kind.to_ascii_lowercase().contains("slink") {
            continue;
        }
        if !matches!(kind, "file" | "dir") {
            continue;
        }
        if name.is_empty() || name.contains(['/', '\\', '\r', '\n', '\0']) || matches!(name, "." | "..") {
            return Err("FTP listing contains an unsafe filename".into());
        }
        let directory = kind == "dir";
        let size = facts.get("size").and_then(|value| value.parse().ok()).unwrap_or(0);
        let modified = facts.get("modify").copied().unwrap_or_default();
        if !directory && (modified.len() < 14 || modified.len() > 24 || !modified.chars().all(|c| c.is_ascii_digit() || c == '.')) {
            return Err("FTP file is missing its MLSD modification time; stable version detection is unavailable".into());
        }
        let unique = facts.get("unique").copied().unwrap_or_default();
        result.push(Entry { name: name.into(), directory, size, revision: format!("{modified}:{unique}") });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::Mutex;

    type ServerFiles = Arc<Mutex<BTreeMap<String, Vec<u8>>>>;

    #[test]
    fn real_tls_verifies_trust_and_hostname_and_enforces_deadline() {
        let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let cert = generated.cert.der().clone();
        let server = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], rustls::pki_types::PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der()).into())
            .unwrap();
        let server = Arc::new(server);
        for (trust, host, succeeds) in [(true, "localhost", true), (false, "localhost", false), (true, "other.example", false)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let config = server.clone();
            let thread = std::thread::spawn(move || {
                let (socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                socket.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut tls = StreamOwned::new(rustls::ServerConnection::new(config).unwrap(), socket);
                let mut bytes = [0_u8; 4];
                if tls.read_exact(&mut bytes).is_ok() {
                    assert_eq!(&bytes, b"ping");
                    tls.write_all(b"pong").unwrap();
                    tls.flush().unwrap();
                }
            });
            let mut roots = RootCertStore::empty();
            if trust {
                roots.add(cert.clone()).unwrap();
            }
            let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let socket = TcpStream::connect(address).unwrap();
            let mut result = secure(socket, host, Arc::new(config), &Control::new(&|| false));
            assert_eq!(result.is_ok(), succeeds);
            if let Ok(socket) = &mut result {
                socket.write_all(b"ping").unwrap();
                let mut bytes = [0_u8; 4];
                socket.read_exact(&mut bytes).unwrap();
                assert_eq!(&bytes, b"pong");
            }
            drop(result);
            thread.join().unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut socket = BoundedTcp { socket: TcpStream::connect(listener.local_addr().unwrap()).unwrap(), deadline: Instant::now() };
        assert_eq!(socket.read(&mut [0_u8]).unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    }

    fn fake_server() -> (FtpSource, ServerFiles, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let files = Arc::new(Mutex::new(BTreeMap::from([("/incoming/scan.pdf".to_owned(), b"synthetic PDF bytes".to_vec())])));
        let remote = files.clone();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            socket.write_all(b"220 Synthetic FTP fixture\r\n").unwrap();
            let mut data_listener = None;
            let mut rename = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let command = line.trim_end_matches(['\r', '\n']);
                let (verb, argument) = command.split_once(' ').unwrap_or((command, ""));
                match verb {
                    "USER" => socket.write_all(b"331 Password required\r\n").unwrap(),
                    "PASS" => socket.write_all(b"230 Logged in\r\n").unwrap(),
                    "TYPE" | "OPTS" => socket.write_all(b"200 Accepted\r\n").unwrap(),
                    "EPSV" => {
                        let data = TcpListener::bind("127.0.0.1:0").unwrap();
                        write!(socket, "229 Entering (|||{}|)\r\n", data.local_addr().unwrap().port()).unwrap();
                        data_listener = Some(data);
                    }
                    "MLSD" | "RETR" => {
                        socket.write_all(b"150 Data follows\r\n").unwrap();
                        let (mut data, _) = data_listener.take().unwrap().accept().unwrap();
                        if verb == "MLSD" {
                            for (path, bytes) in remote.lock().unwrap().iter() {
                                let (parent, name) = path.rsplit_once('/').unwrap();
                                if parent == argument.trim_end_matches('/') {
                                    write!(data, "type=file;size={};modify=20261011000000; {name}\r\n", bytes.len()).unwrap();
                                }
                            }
                        } else {
                            data.write_all(remote.lock().unwrap().get(argument).unwrap()).unwrap();
                        }
                        drop(data);
                        socket.write_all(b"226 Complete\r\n").unwrap();
                    }
                    "DELE" => {
                        remote.lock().unwrap().remove(argument);
                        socket.write_all(b"250 Deleted\r\n").unwrap();
                    }
                    "RNFR" => {
                        rename = argument.into();
                        socket.write_all(b"350 Rename target required\r\n").unwrap();
                    }
                    "RNTO" => {
                        let mut files = remote.lock().unwrap();
                        let bytes = files.remove(&rename).unwrap();
                        files.insert(argument.into(), bytes);
                        socket.write_all(b"250 Renamed\r\n").unwrap();
                    }
                    "AUTH" => {
                        socket.write_all(b"534 TLS unavailable\r\n").unwrap();
                    }
                    _ => socket.write_all(b"502 Unsupported\r\n").unwrap(),
                }
            }
        });
        (
            FtpSource {
                host: address.ip().to_string(),
                port: address.port(),
                security: FtpSecurity::Plain,
                username: "fixture".into(),
                credential_id: "fixture".into(),
                folder: "/incoming".into(),
                ..Default::default()
            },
            files,
            thread,
        )
    }

    #[test]
    fn real_passive_ftp_socket_stages_and_verifies_before_idempotent_move() {
        let (source, remote, thread) = fake_server();
        let control = Control::new(&|| false);
        let mut client = Client::connect_password(&source, "fixture password", &control).unwrap();
        let file = client.list(&control).unwrap().remove(0);
        let mut bytes = Vec::new();
        client.read(&file, &mut bytes, &control).unwrap();
        assert_eq!(bytes, b"synthetic PDF bytes");
        let record = Record {
            source: StagedSource {
                source_id: "fixture".into(),
                relative: "scan.pdf".into(),
                staged_relative: "stable-scan.pdf".into(),
                sha256: hash(&bytes),
            },
            remote: file,
            ready: true,
            acknowledged: false,
        };
        assert!(remote.lock().unwrap().contains_key("/incoming/scan.pdf"));
        client.finish(std::slice::from_ref(&record), &SourceAction::Move { destination: "/archive".into() }, &control).unwrap();
        assert!(!remote.lock().unwrap().contains_key("/incoming/scan.pdf"));
        assert_eq!(remote.lock().unwrap().get("/archive/stable-scan.pdf").unwrap(), b"synthetic PDF bytes");
        client.finish(&[record], &SourceAction::Move { destination: "/archive".into() }, &control).unwrap();
        drop(client);
        thread.join().unwrap();
    }
    #[test]
    fn ftps_never_falls_back_to_plain_ftp_and_changed_remote_data_is_retained() {
        let (mut source, _, thread) = fake_server();
        source.security = FtpSecurity::ExplicitTls;
        assert!(Client::connect_password(&source, "fixture", &Control::new(&|| false)).is_err());
        thread.join().unwrap();
        let (source, remote, thread) = fake_server();
        let control = Control::new(&|| false);
        let mut client = Client::connect_password(&source, "fixture", &control).unwrap();
        let file = client.list(&control).unwrap().remove(0);
        let record = Record {
            source: StagedSource {
                source_id: "fixture".into(),
                relative: "scan.pdf".into(),
                staged_relative: "stable-scan.pdf".into(),
                sha256: hash(b"synthetic PDF bytes"),
            },
            remote: file,
            ready: true,
            acknowledged: false,
        };
        remote.lock().unwrap().insert("/incoming/scan.pdf".into(), b"different PDF bytes".to_vec());
        assert!(client.finish(&[record], &SourceAction::Delete, &control).is_err());
        assert!(remote.lock().unwrap().contains_key("/incoming/scan.pdf"));
        drop(client);
        thread.join().unwrap();
    }
    #[test]
    fn passive_ports_never_use_the_server_supplied_host() {
        assert_eq!(epsv_port("229 Entering (|||12345|)\r\n").unwrap(), 12345);
        assert_eq!(pasv_port("227 Entering (10,0,0,1,4,1)\r\n").unwrap(), 1025);
        for value in ["(|||0|)", "(|||65536|)", "(|||1|x)", "no tuple"] {
            assert!(epsv_port(value).is_err());
        }
    }
    #[test]
    fn listing_requires_stable_versions_and_rejects_paths_and_links() {
        let entries =
            parse_listing(b"type=file;size=123;modify=20261011000000;unique=x; scan.pdf\r\ntype=dir; sub\r\ntype=OS.unix=slink; ignore.pdf\r\n")
                .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].size, 123);
        assert_eq!(entries[0].revision, "20261011000000:x");
        assert!(parse_listing(b"type=file;size=1; missing.pdf\r\n").is_err());
        assert!(parse_listing(b"type=file;size=1;modify=20261011000000; ../bad.pdf\r\n").is_err());
        assert!(remote_path("/safe/../bad").is_err());
    }
}
