//! Just enough HTTP for the cabinet contract: one request in, one response out,
//! and a small plain-`http` client for the commit callback and a source URL.
//!
//! TLS is not implemented here. The daemon is expected to sit behind a reverse
//! proxy (RFC §16), so `https://` URLs are refused with a message that says so
//! rather than silently failing the handshake.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// A request head bigger than this is not the cabinet talking.
const MAX_HEAD: usize = 16 * 1024;
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
    stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| error(format!("could not set a read timeout: {e}")))?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| error(format!("could not clone the stream: {e}")))?);
    let line = read_head_line(&mut reader)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or_else(|| error("the request line has no method"))?.to_owned();
    let path = parts.next().ok_or_else(|| error("the request line has no path"))?.to_owned();
    let mut headers = Vec::new();
    loop {
        let line = read_head_line(&mut reader)?;
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

fn read_head_line(reader: &mut impl BufRead) -> Result<String, HttpError> {
    let mut line = String::new();
    let read = reader.take(MAX_HEAD as u64).read_line(&mut line).map_err(|e| error(format!("could not read the request head: {e}")))?;
    if read == 0 {
        return Err(error("the request head ended early"));
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
pub fn parse_http_url(url: &str) -> Result<Endpoint, HttpError> {
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
        let line = read_head_line(&mut reader)?;
        let mut parts = line.split_whitespace();
        let version = parts.next().ok_or_else(|| error("the response has no status line"))?;
        if !version.starts_with("HTTP/1.") {
            return Err(error("the response is not HTTP/1.x"));
        }
        let code = parts.next().ok_or_else(|| error("the response has no status code"))?;
        let code = code.parse::<u16>().map_err(|_| error("the status code is not a number"))?;
        let mut length = None;
        loop {
            let line = read_head_line(&mut reader)?;
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

    /// `POST` a JSON body and return the status and the response body.
    pub fn post_json_body(&self, url: &str, bearer: Option<&str>, body: &[u8]) -> Result<(u16, Vec<u8>), HttpError> {
        let endpoint = parse_http_url(url)?;
        let mut stream = self.connect(&endpoint)?;
        let auth = match bearer {
            Some(token) => format!("Authorization: Bearer {token}\r\n"),
            None => String::new(),
        };
        let head = format!("POST {} HTTP/1.1\r\nContent-Type: application/json\r\n{auth}", endpoint.path);
        self.exchange(&mut stream, &endpoint, &head, body, DRAIN_LIMIT)
    }

    /// `POST` a JSON body and return the status code. A callback only says yes
    /// or no, so the response body is dropped.
    pub fn post_json(&self, url: &str, bearer: Option<&str>, body: &[u8]) -> Result<u16, HttpError> {
        self.post_json_body(url, bearer, body).map(|(status, _)| status)
    }
}

#[cfg(test)]
mod tests {
    use super::{Client, Endpoint, parse_http_url, read_request, respond};
    use std::io::Read;
    use std::net::TcpListener;
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
        let status = client.post_json(&format!("http://{address}/commit"), None, br#"{"fileId":"file-1"}"#)?;
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
        assert!(client.post_json("http://127.0.0.1:1/commit", None, b"{}").is_err());
    }
}
