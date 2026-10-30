//! server.rs — Secure loopback HTTP server for on-premise document processing.
//! ==============================================================================
//!
//! Purpose:
//!     Binds to 127.0.0.1 loopback interface, decodes HTTP/1.1 requests, enforces
//!     public binding confirmations and optional Bearer auth, invokes RestService,
//!     and streams RFC 7807 responses.
//!
//! Layer:
//!     REST Service / HTTP Server Transport Layer
//!
//! Key Input Dependencies:
//!     - std::net::{TcpListener, TcpStream, SocketAddr}
//!     - crate::routes::{HttpRequest, HttpResponse, RestService}
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_rest::server::{HttpServer, ServerConfig};
//!     // let server = HttpServer::start(service, ServerConfig::default())?;
//!     ```
//!
//! Key Types & Functions Index:
//!     - ServerConfig: Server configuration (bind_addr, allow_public_bind)
//!     - HttpServer: Running HTTP server handle with background listener thread
//!     - HttpServer::start: Starts server on background thread with loopback security check
//!     - HttpServer::local_addr: Bound local socket address
//!     - HttpServer::port: Port number of running server
//!     - HttpServer::stop: Gracefully stops the listener thread

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::routes::{HttpRequest, RestService};

/// Server configuration options.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: SocketAddr,
    pub allow_public_bind: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { bind_addr: "127.0.0.1:0".parse().unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 0))), allow_public_bind: false }
    }
}

/// Running HTTP server handle.
pub struct HttpServer {
    listener_addr: SocketAddr,
    running: Arc<AtomicBool>,
    join_handle: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

struct ConnectionPermit(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl HttpServer {
    /// Starts the HTTP server on a background thread.
    pub fn start(service: Arc<RestService>, config: ServerConfig) -> std::io::Result<Self> {
        // Enforce loopback security policy (§6.1)
        if !config.bind_addr.ip().is_loopback() && !config.allow_public_bind {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Refusing non-loopback bind without explicit public bind confirmation (--public-bind-confirm)",
            ));
        }

        let listener = TcpListener::bind(config.bind_addr)?;
        listener.set_nonblocking(true)?;
        let listener_addr = listener.local_addr()?;
        let running = Arc::new(AtomicBool::new(true));

        let active_conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let is_running = Arc::clone(&running);
        let join_handle = std::thread::Builder::new().name("pdfcraft-rest-http".into()).spawn(move || {
            while is_running.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        if active_conns.load(Ordering::Relaxed) >= MAX_CONCURRENT_CONNECTIONS {
                            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                            let _ = stream
                                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                            continue;
                        }
                        active_conns.fetch_add(1, Ordering::Relaxed);
                        let permit = ConnectionPermit(Arc::clone(&active_conns));
                        let s = Arc::clone(&service);
                        std::thread::Builder::new()
                            .name("pdfcraft-http-conn".into())
                            .spawn(move || {
                                let _permit = permit;
                                handle_connection(stream, &s);
                            })
                            .ok();
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        })?;

        Ok(Self { listener_addr, running, join_handle: std::sync::Mutex::new(Some(join_handle)) })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.listener_addr
    }

    pub fn port(&self) -> u16 {
        self.listener_addr.port()
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.join_handle.lock().ok().and_then(|mut lock| lock.take()) {
            let _ = handle.join();
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Largest request body accepted (matches the SDK's default `max_request_bytes`).
const MAX_BODY_BYTES: usize = 100 * 1024 * 1024;
/// Most request headers accepted.
const MAX_HEADERS: usize = 100;
/// Maximum concurrent active HTTP connection threads.
const MAX_CONCURRENT_CONNECTIONS: usize = 64;

fn write_error(writer: &mut TcpStream, response: &[u8]) {
    let _ = writer.write_all(response);
    let _ = writer.flush();
    let _ = writer.shutdown(std::net::Shutdown::Write);
}

fn handle_connection(stream: TcpStream, service: &RestService) {
    // BSD accepted sockets can inherit the listener's nonblocking mode. Workers
    // use blocking I/O with deadlines, so an idle peer must not become WouldBlock.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let Ok(stream_clone) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream_clone);
    let mut writer = stream;

    let mut line = String::new();
    if reader.by_ref().take(8192).read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }
    if !line.ends_with('\n') {
        write_error(&mut writer, b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }

    // Parse request line: METHOD URI HTTP/1.x (RFC 7230 §3.1.1)
    let trimmed_line = line.strip_suffix("\r\n").or_else(|| line.strip_suffix('\n')).unwrap_or(&line);
    let parts: Vec<&str> = trimmed_line.split(' ').collect();
    if parts.len() != 3 || parts[0].is_empty() || parts[1].is_empty() {
        write_error(&mut writer, b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();
    let proto = parts[2];
    if proto != "HTTP/1.1" && proto != "HTTP/1.0" {
        write_error(&mut writer, b"HTTP/1.1 505 HTTP Version Not Supported\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }

    // Parse headers
    let mut headers = HashMap::new();
    let mut content_length = 0usize;
    let mut seen_content_length = false;
    let mut total_header_lines = 0usize;
    let mut total_header_bytes = 0usize;
    const MAX_HEADER_BYTES: usize = 64 * 1024;

    loop {
        total_header_lines = total_header_lines.saturating_add(1);
        if total_header_lines > MAX_HEADERS || total_header_bytes > MAX_HEADER_BYTES {
            write_error(&mut writer, b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        line.clear();
        match reader.by_ref().take(8192).read_line(&mut line) {
            Ok(0) => {
                // Unexpected EOF before end of headers (RFC 7230 framing violation)
                write_error(&mut writer, b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                return;
            }
            Err(_) => {
                write_error(&mut writer, b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                return;
            }
            Ok(_) => {}
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if line.trim().is_empty() {
            // Whitespace-only line is invalid obs-fold / malformed header
            write_error(&mut writer, b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        if !line.ends_with('\n') {
            write_error(&mut writer, b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
        total_header_bytes = total_header_bytes.saturating_add(line.len());
        if let Some((k, v)) = line.split_once(':') {
            // RFC 7230 §3.2.4: field-name = token, no whitespace allowed around field name
            let valid_field_name = !k.is_empty()
                && k.bytes().all(|b| {
                    b.is_ascii_alphanumeric()
                        || matches!(b, b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~')
                });
            if !valid_field_name {
                write_error(&mut writer, b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                return;
            }
            let key = k.to_lowercase();
            let val = v.trim().to_string();

            // Reject unsupported Transfer-Encoding (chunked not supported, preventing framing ambiguities)
            if key == "transfer-encoding" {
                write_error(&mut writer, b"HTTP/1.1 501 Not Implemented\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                return;
            }

            if key == "content-length" {
                if seen_content_length {
                    // Prevent HTTP Request Smuggling via duplicate Content-Length headers (RFC 7230 §3.3.2)
                    write_error(&mut writer, b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    return;
                }
                seen_content_length = true;
                match val.parse::<usize>() {
                    Ok(len) => content_length = len,
                    Err(_) => {
                        write_error(&mut writer, b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        return;
                    }
                }
            }
            headers.insert(key, val);
        } else {
            write_error(&mut writer, b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
    }

    // Read body. The length comes from the peer, so cap it before allocating.
    if content_length > MAX_BODY_BYTES {
        write_error(&mut writer, b"HTTP/1.1 413 Content Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    let mut body = Vec::with_capacity(content_length.min(64 * 1024));
    if content_length > 0 && (reader.take(content_length as u64).read_to_end(&mut body).is_err() || body.len() != content_length) {
        return;
    }

    let req = HttpRequest { method, path, headers, body };

    let resp = service.dispatch(&req);

    // Write HTTP response
    let status_line = match resp.status {
        200 => "HTTP/1.1 200 OK\r\n",
        202 => "HTTP/1.1 202 Accepted\r\n",
        400 => "HTTP/1.1 400 Bad Request\r\n",
        401 => "HTTP/1.1 401 Unauthorized\r\n",
        404 => "HTTP/1.1 404 Not Found\r\n",
        409 => "HTTP/1.1 409 Conflict\r\n",
        413 => "HTTP/1.1 413 Payload Too Large\r\n",
        429 => "HTTP/1.1 429 Too Many Requests\r\n",
        500 => "HTTP/1.1 500 Internal Server Error\r\n",
        501 => "HTTP/1.1 501 Not Implemented\r\n",
        503 => "HTTP/1.1 503 Service Unavailable\r\n",
        504 => "HTTP/1.1 504 Gateway Timeout\r\n",
        _ => "HTTP/1.1 500 Internal Server Error\r\n",
    };

    let mut response_bytes = Vec::new();
    response_bytes.extend_from_slice(status_line.as_bytes());

    for (k, v) in &resp.headers {
        response_bytes.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    if !resp.headers.keys().any(|k| k.eq_ignore_ascii_case("content-length")) {
        response_bytes.extend_from_slice(format!("Content-Length: {}\r\n", resp.body.len()).as_bytes());
    }
    response_bytes.extend_from_slice(b"Connection: close\r\n\r\n");

    let _ = writer.write_all(&response_bytes);

    match &resp.body {
        crate::routes::HttpBody::Bytes(bytes) => {
            let _ = writer.write_all(bytes);
        }
        crate::routes::HttpBody::File { file, guard, .. } => {
            use std::io::Seek;

            if let Ok(mut reader) = file.lock()
                && reader.rewind().is_ok()
            {
                let _ = std::io::copy(&mut *reader, &mut writer);
            }

            if let Some(guard) = guard {
                let _ = guard.cleanup();
            }
        }
    }
    let _ = writer.flush();
}
