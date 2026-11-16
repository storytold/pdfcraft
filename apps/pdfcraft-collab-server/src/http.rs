//! Just enough HTTP for the cabinet contract: one request in, one response out,
//! and a small plain-`http` client for the commit callback and a source URL.
//!
//! TLS is not implemented here. The daemon is expected to sit behind a reverse
//! proxy (RFC §16), so `https://` URLs are refused with a message that says so
//! rather than silently failing the handshake.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// A request head bigger than this is not the cabinet talking. The budget is for
/// the whole head — the request line and every header line — not for one line:
/// a hundred small headers are as much of an attack as one huge one.
const MAX_HEAD: usize = 16 * 1024;
/// How long the whole head may take to arrive. A per-read timeout alone does not
/// bound it: a peer that sends one byte just before every timeout keeps a read
/// awake forever.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of a response body is read just to keep the connection clean.
const DRAIN_LIMIT: u64 = 64 * 1024;

#[derive(Debug)]
pub struct HttpError(pub String);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HttpError {}

fn error(message: impl Into<String>) -> HttpError {
    HttpError(message.into())
}

pub struct Request {
    pub method: String,
    pub path: String,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }
}

/// Read one request from `stream`. `max_body` is the largest body accepted; a
/// bigger `Content-Length` is refused before a single byte is buffered.
pub fn read_request(stream: &mut TcpStream, max_body: usize) -> Result<Request, HttpError> {
    stream.set_read_timeout(Some(HEAD_TIMEOUT)).map_err(|e| error(format!("could not set a read timeout: {e}")))?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| error(format!("could not clone the stream: {e}")))?);
    // One budget and one deadline for the whole head: every line spends from the
    // same 16 KiB and the same ten seconds.
    let mut budget = MAX_HEAD;
    let deadline = Instant::now() + HEAD_TIMEOUT;
    let line = read_head_line(&mut reader, "request", &mut budget, deadline)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or_else(|| error("the request line has no method"))?.to_owned();
    let path = parts.next().ok_or_else(|| error("the request line has no path"))?.to_owned();
    let mut headers = Vec::new();
    loop {
        let line = read_head_line(&mut reader, "request", &mut budget, deadline)?;
        if line.trim().is_empty() {
            break;
        }
        let (key, value) = line.split_once(':').ok_or_else(|| error("a header line has no colon"))?;
        headers.push((key.trim().to_owned(), value.trim().to_owned()));
    }
    let request = Request { method, path, headers, body: Vec::new() };
    if request.header("transfer-encoding").is_some() {
        return Err(error("chunked bodies are not supported; send Content-Length"));
    }
    let length = match request.header("content-length") {
        Some(value) => value.parse::<usize>().map_err(|_| error("Content-Length is not a number"))?,
        None => 0,
    };
    if length > max_body {
        return Err(error(format!("the body is larger than the {max_body} byte limit")));
    }
    if request.header("expect").is_some_and(|value| value.eq_ignore_ascii_case("100-continue")) {
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").map_err(|e| error(format!("could not answer 100-continue: {e}")))?;
        stream.flush().map_err(|e| error(format!("could not flush 100-continue: {e}")))?;
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).map_err(|e| error(format!("could not read the body: {e}")))?;
    Ok(Request { body, ..request })
}

/// Read one head line, spending at most `budget` bytes of the head as a whole and
/// no more than `deadline` of wall-clock time. `what` names the head in the
/// error ("request" or "response"): the message goes to a log and has to say
/// which side was refused.
fn read_head_line(reader: &mut impl BufRead, what: &str, budget: &mut usize, deadline: Instant) -> Result<String, HttpError> {
    if *budget == 0 {
        return Err(error(format!("the {what} head is larger than the {MAX_HEAD} byte limit")));
    }
    if Instant::now() >= deadline {
        return Err(error(format!("the {what} head took longer than the {} second deadline", HEAD_TIMEOUT.as_secs())));
    }
    let allowed = *budget;
    let mut line = String::new();
    let read = reader.take(allowed as u64).read_line(&mut line).map_err(|e| error(format!("could not read the {what} head: {e}")))?;
    if read == 0 {
        return Err(error(format!("the {what} head ended early")));
    }
    *budget = budget.saturating_sub(read);
    // A line cut off at the budget is not a line the parser may see: refusing it
    // here is what keeps a truncated header from being read as a complete one.
    if *budget == 0 && !line.ends_with('\n') {
        return Err(error(format!("the {what} head is larger than the {MAX_HEAD} byte limit")));
    }
    Ok(line)
}

/// Write a complete response and close the connection.
pub fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) -> std::io::Result<()> {
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
    stream.write_all(body)?;
    stream.flush()
}

/// The parts of a `http://` URL the client needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub path: String,
}

/// Parse an `http://` URL. `https://` is refused: this build has no TLS client,
/// and a silent downgrade would send a session token in the clear.
///
/// Only visible ASCII is accepted. The path is interpolated into a request line
/// verbatim, so a space would end the target and a CR or LF would start a header
/// of the caller's choosing: a URL that carries either is refused, not escaped.
pub fn parse_http_url(url: &str) -> Result<Endpoint, HttpError> {
    if let Some(byte) = url.bytes().find(|byte| !(0x21..=0x7e).contains(byte)) {
        return Err(error(format!("the URL carries {byte:#04x}, which a request line cannot: only visible ASCII is supported")));
    }
    if let Some(rest) = url.strip_prefix("https://") {
        let _ = rest;
        return Err(error("TLS is not supported by this build; use an http:// URL behind the reverse proxy"));
    }
    let rest = url.strip_prefix("http://").ok_or_else(|| error("only http:// URLs are supported"))?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(error("the URL has no host"));
    }
    if authority.contains('@') {
        return Err(error("credentials in a URL are not supported"));
    }
    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        let (host, after) = stripped.split_once(']').ok_or_else(|| error("the IPv6 host is not closed"))?;
        (host.to_owned(), after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port)),
            None => (authority.to_owned(), None),
        }
    };
    if host.is_empty() {
        return Err(error("the URL has no host"));
    }
    let port = match port {
        Some(port) => port.parse::<u16>().map_err(|_| error("the URL port is not a number"))?,
        None => 80,
    };
    let path = if path.starts_with('/') { path.to_owned() } else { format!("/{path}") };
    Ok(Endpoint { host, port, path })
}

/// A plain-HTTP client with one deadline per request. No redirects, no proxy,
/// no chunked responses: a callback endpoint that needs those is not one this
/// daemon can talk to, and refusing is better than guessing.
pub struct Client {
    pub timeout: Duration,
}

impl Client {
    fn connect(&self, endpoint: &Endpoint) -> Result<TcpStream, HttpError> {
        let addresses =
            (endpoint.host.as_str(), endpoint.port).to_socket_addrs().map_err(|e| error(format!("could not resolve {}: {e}", endpoint.host)))?;
        let mut last = error(format!("no address for {}", endpoint.host));
        for address in addresses {
            match TcpStream::connect_timeout(&address, self.timeout) {
                Ok(stream) => {
                    let _ = stream.set_read_timeout(Some(self.timeout));
                    let _ = stream.set_write_timeout(Some(self.timeout));
                    return Ok(stream);
                }
                Err(e) => last = error(format!("could not connect to {address}: {e}")),
            }
        }
        Err(last)
    }

    fn exchange(&self, stream: &mut TcpStream, endpoint: &Endpoint, head: &str, body: &[u8], limit: u64) -> Result<(u16, Vec<u8>), HttpError> {
        stream
            .write_all(format!("{head}Host: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", endpoint.host, body.len()).as_bytes())
            .map_err(|e| error(format!("could not send the request: {e}")))?;
        stream.write_all(body).map_err(|e| error(format!("could not send the body: {e}")))?;
        stream.flush().map_err(|e| error(format!("could not flush the request: {e}")))?;
        let mut reader = BufReader::new(stream.try_clone().map_err(|e| error(format!("could not clone the stream: {e}")))?);
        let mut budget = MAX_HEAD;
        let deadline = Instant::now() + self.timeout;
        let line = read_head_line(&mut reader, "response", &mut budget, deadline)?;
        let mut parts = line.split_whitespace();
        let version = parts.next().ok_or_else(|| error("the response has no status line"))?;
        if !version.starts_with("HTTP/1.") {
            return Err(error("the response is not HTTP/1.x"));
        }
        let code = parts.next().ok_or_else(|| error("the response has no status code"))?;
        let code = code.parse::<u16>().map_err(|_| error("the status code is not a number"))?;
        let mut length = None;
        loop {
            let line = read_head_line(&mut reader, "response", &mut budget, deadline)?;
            if line.trim().is_empty() {
                break;
            }
            if let Some((key, value)) = line.split_once(':')
                && key.trim().eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<u64>().ok();
            }
        }
        let mut response = Vec::new();
        match length {
            Some(length) if length > limit => return Err(error(format!("the response is larger than the {limit} byte limit"))),
            Some(length) => {
                response.resize(usize::try_from(length).map_err(|_| error("the response is too large"))?, 0);
                reader.read_exact(&mut response).map_err(|e| error(format!("could not read the response: {e}")))?;
            }
            None => {
                reader.take(limit).read_to_end(&mut response).map_err(|e| error(format!("could not read the response: {e}")))?;
            }
        }
        Ok((code, response))
    }

    /// `GET` a URL, reading at most `limit` bytes.
    pub fn get(&self, url: &str, limit: usize) -> Result<(u16, Vec<u8>), HttpError> {
        let endpoint = parse_http_url(url)?;
        let mut stream = self.connect(&endpoint)?;
        let head = format!("GET {} HTTP/1.1\r\nAccept: application/pdf\r\n", endpoint.path);
        self.exchange(&mut stream, &endpoint, &head, &[], limit as u64)
    }

    /// `POST` a JSON body with extra request headers (the commit callback's
    /// `Idempotency-Key`, RFC §8.2) and return the status and the response
    /// body. No redirects, no chunked bodies: the daemon's cabinet is known.
    pub fn post_json(&self, url: &str, bearer: Option<&str>, headers: &[(&str, &str)], body: &[u8]) -> Result<(u16, Vec<u8>), HttpError> {
        let endpoint = parse_http_url(url)?;
        let mut stream = self.connect(&endpoint)?;
        let auth = match bearer {
            Some(token) => format!("Authorization: Bearer {token}\r\n"),
            None => String::new(),
        };
        let mut extra = String::new();
        for (name, value) in headers {
            extra.push_str(&format!("{name}: {value}\r\n"));
        }
        let head = format!("POST {} HTTP/1.1\r\nContent-Type: application/json\r\n{auth}{extra}", endpoint.path);
        self.exchange(&mut stream, &endpoint, &head, body, DRAIN_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::{Client, Endpoint, parse_http_url, read_request, respond};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    #[test]
    fn only_plain_http_urls_parse() {
        assert_eq!(
            parse_http_url("http://127.0.0.1:8788/commit").expect("a valid URL"),
            Endpoint { host: "127.0.0.1".into(), port: 8788, path: "/commit".into() }
        );
        assert_eq!(parse_http_url("http://example.test").expect("a valid URL").path, "/");
        assert_eq!(parse_http_url("http://[::1]:9/x").expect("a valid IPv6 URL").host, "::1");
        assert!(parse_http_url("https://cabinet.example/commit").is_err(), "no TLS client is shipped");
        assert!(parse_http_url("ftp://example.test/commit").is_err());
        assert!(parse_http_url("http://user:pass@example.test/").is_err());
        assert!(parse_http_url("http:///nohost").is_err());
    }

    /// A URL is pasted into a request line as it stands, so a space would end the
    /// target early and a CRLF would start a header of the caller's choosing
    /// (thread #10 on PR #6622).
    #[test]
    fn a_url_cannot_carry_a_space_a_line_break_or_a_control_character() {
        for url in [
            "http://127.0.0.1:8788/a b",
            "http://127.0.0.1:8788/a\r\nX-Injected: 1",
            "http://127.0.0.1:8788/a\n",
            "http://127.0.0.1:8788/a\0",
            "http://127.0.0.1:8788/\u{7f}",
            "http://127.0.0.1:8788/caf\u{e9}",
        ] {
            assert!(parse_http_url(url).is_err(), "{url:?} must be refused before it reaches a request");
        }
        // The ordinary shapes still parse.
        assert_eq!(parse_http_url("http://127.0.0.1:8788/file.pdf").expect("a plain URL").path, "/file.pdf");
        assert_eq!(parse_http_url("http://cabinet.test/a/b?c=d").expect("a query is part of the target").path, "/a/b?c=d");
    }

    /// A hundred small headers are as much of an attack as one huge line: the
    /// limit is the whole head, not one line (thread #9 on PR #6622).
    #[test]
    fn a_request_head_is_bounded_as_a_whole_not_line_by_line() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            if stream.write_all(b"GET / HTTP/1.1\r\n").is_err() {
                return;
            }
            for i in 0..64 {
                let header = format!("X-Pad-{i}: {}\r\n", "a".repeat(1000));
                if stream.write_all(header.as_bytes()).is_err() {
                    return;
                }
            }
            let _ = stream.flush();
            // Hold the socket open: the reader has to refuse the head on its own.
            std::thread::sleep(Duration::from_millis(300));
        });
        let mut stream = TcpStream::connect(address)?;
        let error = read_request(&mut stream, 1 << 20).err().ok_or("a 64 KiB of small headers was accepted")?;
        assert!(error.0.contains("head is larger than the"), "{error}");
        Ok(())
    }

    /// A one-request server on loopback; returns its address and the request it
    /// read.
    fn one_shot(reply: &'static str) -> std::io::Result<(std::net::SocketAddr, std::sync::mpsc::Receiver<String>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let request = read_request(&mut stream, 1 << 20)
                .map(|request| format!("{} {}\n{}", request.method, request.path, String::from_utf8_lossy(&request.body)));
            let body = format!("{}\n", request.unwrap_or_else(|e| format!("error: {e}")));
            let _ = respond(&mut stream, reply, "text/plain", body.as_bytes());
            let _ = sender.send(body);
        });
        Ok((address, receiver))
    }

    #[test]
    fn a_posted_json_body_and_its_status_arrive() -> Result<(), Box<dyn std::error::Error>> {
        let (address, receiver) = one_shot("200 OK")?;
        let client = Client { timeout: Duration::from_secs(5) };
        let (status, _) = client.post_json(&format!("http://{address}/commit"), None, &[], br#"{"fileId":"file-1"}"#)?;
        assert_eq!(status, 200);
        let seen = receiver.recv()?;
        assert!(seen.starts_with("POST /commit"), "{seen}");
        assert!(seen.contains(r#"{"fileId":"file-1"}"#), "{seen}");
        Ok(())
    }

    #[test]
    fn a_get_reads_the_body_and_a_big_one_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        std::thread::spawn(move || {
            for _ in 0..2 {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut head = [0u8; 1024];
                let _ = stream.read(&mut head);
                let body = b"%PDF-1.7 fixture";
                let _ = respond(&mut stream, "200 OK", "application/pdf", body);
            }
        });
        let client = Client { timeout: Duration::from_secs(5) };
        let (status, body) = client.get(&format!("http://{address}/file.pdf"), 1024)?;
        assert_eq!(status, 200);
        assert_eq!(body, b"%PDF-1.7 fixture");
        assert!(client.get(&format!("http://{address}/file.pdf"), 4).is_err(), "the size cap is enforced");
        Ok(())
    }

    #[test]
    fn a_refused_connection_is_an_error_not_a_hang() {
        let client = Client { timeout: Duration::from_millis(200) };
        assert!(client.post_json("http://127.0.0.1:1/commit", None, &[], b"{}").is_err());
    }
}
