//! A loopback HTTP server for tests (feature `test-server`): it answers every POST with what a
//! closure returns, so a test can play a time server without the network. Bound to 127.0.0.1 on a
//! free port; it serves until the test process ends.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The largest request read (bytes): an RFC 3161 request is about a hundred.
const MAX_REQUEST: usize = 64 * 1024;

/// A running test server.
pub struct TestServer {
    /// `http://127.0.0.1:<port>/tsa`.
    pub url: String,
    requests: Arc<AtomicUsize>,
}

impl TestServer {
    /// How many requests it has answered.
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

/// Serve on loopback, answering each request body with `answer`'s (HTTP status, body).
pub fn serve(answer: impl Fn(&[u8]) -> (u16, Vec<u8>) + Send + 'static) -> std::io::Result<TestServer> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://{}/tsa", listener.local_addr()?);
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if let Some(body) = read_request(&stream) {
                let (status, reply) = answer(&body);
                counter.fetch_add(1, Ordering::SeqCst);
                let head = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/timestamp-reply\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.len()
                );
                let mut stream = stream;
                let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(&reply)).and_then(|()| stream.flush());
            }
        }
    });
    Ok(TestServer { url, requests })
}

/// An address on loopback where nothing listens: a port that was free a moment ago.
pub fn unreachable_url() -> std::io::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://{}/tsa", listener.local_addr()?);
    drop(listener);
    Ok(url)
}

/// The body of one HTTP/1.1 request with a `Content-Length` (what the client sends).
fn read_request(mut stream: &TcpStream) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(data.get(..end)?).to_ascii_lowercase();
            let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
            let start = end + 4;
            while data.len() < start.checked_add(length)?.min(MAX_REQUEST) {
                let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
                data.extend_from_slice(chunk.get(..n)?);
            }
            return data.get(start..start.checked_add(length)?).map(<[u8]>::to_vec);
        }
        if data.len() > MAX_REQUEST {
            return None;
        }
        let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
        data.extend_from_slice(chunk.get(..n)?);
    }
}
