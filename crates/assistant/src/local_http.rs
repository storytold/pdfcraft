//! A small HTTP/1.1 client for a model server on this computer (plain `http://` to a loopback
//! address), whose one job is to stop at once when cancelled.
//!
//! A general client blocks while a large model loads, and nothing arrives to check a cancel flag
//! against. Here every read waits at most [`TICK`]: when the person cancels, the socket is closed
//! right away, the server sees the client gone and abandons the load or the generation, and only
//! then does the caller finish the request. External (`https://`) providers keep using ureq.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Longest wait of one read before the cancel flag is checked again.
pub const TICK: Duration = Duration::from_millis(500);

pub const CANCELLED: &str = "Cancelado.";

/// Reads that give up when `cancel` is set or the deadline passes.
struct Watched<'a> {
    stream: TcpStream,
    cancel: &'a AtomicBool,
    deadline: Instant,
    read: u64,
    limit: u64,
}

impl Read for Watched<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                // Closing both ways tells the server at once that nobody is listening.
                let _ = self.stream.shutdown(std::net::Shutdown::Both);
                // Not `Interrupted`: the standard readers retry that kind forever.
                return Err(std::io::Error::new(ErrorKind::ConnectionAborted, CANCELLED));
            }
            if Instant::now() >= self.deadline {
                return Err(std::io::Error::new(ErrorKind::TimedOut, "el modelo tardó demasiado"));
            }
            match self.stream.read(buf) {
                Ok(n) => {
                    self.read = self.read.saturating_add(n as u64);
                    if self.read > self.limit {
                        return Err(std::io::Error::other("respuesta demasiado grande"));
                    }
                    return Ok(n);
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

/// The decoded body of a `Transfer-Encoding: chunked` response.
struct Chunked<R> {
    inner: R,
    left: u64,
    done: bool,
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let mut line = String::new();
            self.inner.read_line(&mut line)?;
            if line.trim().is_empty() {
                // The CRLF after the previous chunk.
                line.clear();
                self.inner.read_line(&mut line)?;
            }
            let size = line.trim().split(';').next().unwrap_or("");
            self.left = u64::from_str_radix(size, 16).map_err(|_| std::io::Error::other("respuesta HTTP mal formada"))?;
            if self.left == 0 {
                self.done = true;
                return Ok(0);
            }
        }
        let want = usize::try_from(self.left).unwrap_or(usize::MAX).min(buf.len());
        let n = self.inner.read(buf.get_mut(..want).unwrap_or(&mut []))?;
        if n == 0 {
            return Err(std::io::Error::new(ErrorKind::UnexpectedEof, "la respuesta se cortó"));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// POST `body` (JSON) to `url` and hand each line of the reply to `line`, until it returns false,
/// the reply ends, `cancel` is set (the connection is closed) or `timeout` passes.
pub fn post_lines(
    url: &str,
    body: &str,
    api_key: Option<&str>,
    timeout: Duration,
    limit: u64,
    cancel: &AtomicBool,
    mut line: impl FnMut(&str) -> Result<bool, String>,
) -> Result<(), String> {
    let rest = url.strip_prefix("http://").ok_or("sólo http:// local")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("/")),
        None => (rest, "/"),
    };
    let target = if authority.contains(':') { authority.to_owned() } else { format!("{authority}:80") };
    let addrs: Vec<SocketAddr> = target.to_socket_addrs().map_err(|e| format!("no se pudo resolver {authority}: {e}"))?.collect();
    // Only this computer: whatever a name resolves to, nothing else is contacted here.
    let addr = addrs.into_iter().find(|a| a.ip().is_loopback()).ok_or_else(|| format!("{authority} no es una dirección de esta Mac"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| format!("no se pudo conectar con {authority}: {e}"))?;
    stream.set_read_timeout(Some(TICK)).map_err(|e| e.to_string())?;
    if api_key.is_some_and(|k| k.chars().any(char::is_control)) {
        return Err("Invalid API key header.".into());
    }
    let auth = api_key.filter(|k| !k.is_empty()).map(|k| format!("Authorization: Bearer {k}\r\n")).unwrap_or_default();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\n{auth}Content-Type: application/json\r\nAccept: application/x-ndjson, text/event-stream, application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body.as_bytes())).map_err(|e| format!("no se pudo enviar la consulta: {e}"))?;
    let watched = Watched { stream, cancel, deadline: Instant::now() + timeout, read: 0, limit };
    let mut reader = BufReader::new(watched);
    let io = |e: std::io::Error| {
        if e.kind() == ErrorKind::ConnectionAborted && e.to_string() == CANCELLED {
            CANCELLED.to_owned()
        } else {
            format!("se cortó la respuesta: {e}")
        }
    };
    // Status line and headers.
    let mut status_line = String::new();
    reader.read_line(&mut status_line).map_err(io)?;
    let status: u16 = status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("respuesta HTTP mal formada")?;
    let mut chunked = false;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).map_err(io)? == 0 || h.trim().is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }
    let body_reader: Box<dyn BufRead + '_> =
        if chunked { Box::new(BufReader::new(Chunked { inner: reader, left: 0, done: false })) } else { Box::new(reader) };
    if !(200..300).contains(&status) {
        let mut text = String::new();
        let _ = body_reader.take(4096).read_to_string(&mut text);
        return Err(format!("el servidor respondió {status}: {}", text.chars().take(400).collect::<String>()));
    }
    for l in body_reader.lines() {
        let l = l.map_err(io)?;
        if !line(&l)? {
            break;
        }
    }
    Ok(())
}
