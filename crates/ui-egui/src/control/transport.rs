use super::{ControlClient, Value, json};

// ---- transport (native only) -----------------------------------------------------------------

/// Where a running app's control channel can be reached (written to the `--control` file).
#[cfg(not(target_arch = "wasm32"))]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
}

/// What a connection may do before it authenticates, so an unauthenticated local client cannot
/// hold a thread for ever or exhaust them (#1806).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct Limits {
    /// How long a connection has to authenticate, counted from its acceptance: an absolute
    /// deadline, so a client feeding one byte at a time cannot extend it.
    auth_deadline: std::time::Duration,
    /// How many connections may wait for authentication at once; further ones are refused.
    max_unauthenticated: usize,
    /// How long a write may block on a client that never reads its replies.
    write_timeout: std::time::Duration,
    /// How long a connection refused after a reply is kept half-open, discarding input, so the
    /// reply is not lost to a reset (see [`close_after_reply`]).
    linger: std::time::Duration,
    /// How many refused connections may linger at once. A lingering connection has given back
    /// its place among [`Self::max_unauthenticated`], so this is what bounds their threads;
    /// beyond it a refused connection is closed at once.
    max_lingering: usize,
}

#[cfg(not(target_arch = "wasm32"))]
const LIMITS: Limits = Limits {
    auth_deadline: std::time::Duration::from_secs(10),
    max_unauthenticated: 8,
    write_timeout: std::time::Duration::from_secs(30),
    linger: std::time::Duration::from_secs(1),
    max_lingering: 8,
};

/// One of a limited number of places, given back when dropped: a place among
/// [`Limits::max_unauthenticated`] (given back when the connection authenticates or is refused,
/// and on every way its thread can end), or among [`Limits::max_lingering`].
#[cfg(not(target_arch = "wasm32"))]
struct Slot(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[cfg(not(target_arch = "wasm32"))]
impl Slot {
    fn take(waiting: &std::sync::Arc<std::sync::atomic::AtomicUsize>, max: usize) -> Option<Self> {
        use std::sync::atomic::Ordering;
        let mut n = waiting.load(Ordering::Acquire);
        loop {
            if n >= max {
                return None;
            }
            match waiting.compare_exchange_weak(n, n.saturating_add(1), Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Slot(waiting.clone())),
                Err(now) => n = now,
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// The reading half of a connection, with the authentication deadline: every read before it
/// gets the time left as its timeout, and none at all once it has passed.
#[cfg(not(target_arch = "wasm32"))]
struct Reading {
    stream: std::net::TcpStream,
    until: Option<std::time::Instant>,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::io::Read for Reading {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(until) = self.until {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the authentication deadline passed"));
            }
            self.stream.set_read_timeout(Some(left))?;
        }
        self.stream.read(buf)
    }
}

/// Close a connection whose last reply has just been written without losing that reply.
///
/// Closing a socket with input still unread makes the kernel send a reset instead of a FIN, and
/// the reset can discard the reply before the client reads it (Linux does): a client still
/// sending when it is refused would never learn why. So send FIN first, then read and discard
/// whatever arrives until the client closes its side or `linger` passes, and only then close.
///
/// Only [`Limits::max_lingering`] connections linger at once: `place` is one of those places,
/// taken by [`refuse`] before the reply was written, and held for the drain only. Without one
/// the connection is closed at once after the FIN, accepting the risk of a reset rather than
/// holding another thread.
#[cfg(not(target_arch = "wasm32"))]
fn close_after_reply(reading: &mut Reading, place: Option<Slot>, linger: std::time::Duration) {
    use std::io::Read;
    let _ = reading.stream.shutdown(std::net::Shutdown::Write);
    let Some(_place) = place else { return };
    reading.until = Some(std::time::Instant::now() + linger);
    let mut sink = [0u8; 1024];
    // Each read is bounded by the time left, and none is made once `linger` has passed.
    while matches!(reading.read(&mut sink), Ok(n) if n > 0) {}
}

/// A connection is being refused before `auth`: give back its place among those waiting to
/// authenticate, so it does not keep a real client out while it lingers, and take a place to
/// linger in if one is free. Done before the reply is written, so a client that has read the
/// reply sees the places as they will be.
#[cfg(not(target_arch = "wasm32"))]
fn refuse(slot: &mut Option<Slot>, lingering: &std::sync::Arc<std::sync::atomic::AtomicUsize>, limits: Limits) -> Option<Slot> {
    drop(slot.take());
    Slot::take(lingering, limits.max_lingering)
}

/// Listen on `127.0.0.1` (random port) and forward authenticated requests to `client`.
///
/// Protocol: newline-delimited JSON-RPC 2.0. The first request on a connection must be
/// `{"method": "auth", "params": {"token": …}}`; anything else closes the connection, and so does
/// a line longer than [`MAX_AUTH_LINE`] before that, or [`LIMITS`] passing without it.
#[cfg(not(target_arch = "wasm32"))]
pub fn serve(client: ControlClient) -> std::io::Result<Endpoint> {
    serve_with(client, LIMITS)
}

/// How long the accepting thread waits after a failed accept. Accepting can keep failing, for
/// example while the process has no file descriptors left, and retrying at once would keep a
/// core busy until it recovers. The single-instance listener waits the same way.
#[cfg(not(target_arch = "wasm32"))]
const ACCEPT_RETRY: std::time::Duration = std::time::Duration::from_millis(100);

/// The connections `incoming` accepts, calling `wait` after each failed accept.
#[cfg(not(target_arch = "wasm32"))]
fn accepted<T>(incoming: impl Iterator<Item = std::io::Result<T>>, mut wait: impl FnMut()) -> impl Iterator<Item = T> {
    incoming.filter_map(move |stream| stream.map_err(|_| wait()).ok())
}

#[cfg(not(target_arch = "wasm32"))]
fn serve_with(client: ControlClient, limits: Limits) -> std::io::Result<Endpoint> {
    use std::io::{BufReader, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let token = random_token()?;
    let expected = token.clone();
    let waiting = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let lingering = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    std::thread::Builder::new().name("pdfcraft-control".into()).spawn(move || {
        for stream in accepted(listener.incoming(), || std::thread::sleep(ACCEPT_RETRY)) {
            let _ = stream.set_write_timeout(Some(limits.write_timeout));
            // Admitted only while fewer than the limit wait to authenticate: refused on the
            // accepting thread, so a refused connection costs no thread at all.
            let Some(slot) = Slot::take(&waiting, limits.max_unauthenticated) else {
                let mut write = stream;
                let e = rpc_error(Value::Null, -32003, "too many connections waiting to authenticate; try again later");
                let _ = writeln!(write, "{e}").and_then(|_| write.flush());
                continue;
            };
            let client = client.clone();
            let expected = expected.clone();
            let lingering = lingering.clone();
            let _ = std::thread::Builder::new().name("pdfcraft-control-conn".into()).spawn(move || {
                // Held until `auth` succeeds; given back however this thread ends.
                let mut slot = Some(slot);
                let Ok(read) = stream.try_clone() else { return };
                let mut write = stream;
                let mut authed = false;
                let mut reader = BufReader::new(Reading { stream: read, until: Some(std::time::Instant::now() + limits.auth_deadline) });
                let mut buf = Vec::new();
                loop {
                    // Until `auth` succeeds, lines are capped at MAX_AUTH_LINE; afterwards they
                    // are read whole, as before.
                    let max = if authed { usize::MAX } else { MAX_AUTH_LINE };
                    match read_line_limited(&mut reader, &mut buf, max) {
                        Ok(LineRead::Line) => {}
                        // The deadline passed before `auth`: say so and close.
                        Err(e) if !authed && matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
                            let secs = limits.auth_deadline.as_secs_f64();
                            let e = rpc_error(Value::Null, -32004, &format!("not authenticated within {secs} s"));
                            let place = refuse(&mut slot, &lingering, limits);
                            if writeln!(write, "{e}").and_then(|_| write.flush()).is_ok() {
                                close_after_reply(reader.get_mut(), place, limits.linger);
                            }
                            break;
                        }
                        // End of stream or a failed read ends the connection, as before.
                        Ok(LineRead::End) | Err(_) => break,
                        Ok(LineRead::TooLong) => {
                            let e = rpc_error(Value::Null, -32600, &format!("request line longer than {max} bytes before auth"));
                            let place = refuse(&mut slot, &lingering, limits);
                            if writeln!(write, "{e}").and_then(|_| write.flush()).is_ok() {
                                close_after_reply(reader.get_mut(), place, limits.linger);
                            }
                            break;
                        }
                    }
                    // Bytes that aren't UTF-8 end the connection, as `BufRead::lines` did.
                    let Ok(line) = std::str::from_utf8(&buf) else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let (reply, close) = match serde_json::from_str::<Value>(line) {
                        // Before `auth`, like any other first request, this closes the connection.
                        Err(e) => (rpc_error(Value::Null, -32700, &format!("parse error: {e}")), !authed),
                        Ok(msg) => {
                            let id = msg.get("id").cloned().unwrap_or(Value::Null);
                            let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
                            let params = msg.get("params").cloned().unwrap_or(Value::Null);
                            if !authed {
                                let ok =
                                    method == "auth" && params.get("token").and_then(Value::as_str).is_some_and(|t| constant_time_eq(t, &expected));
                                authed = ok;
                                if ok {
                                    // Authenticated: no deadline from here on, and its place is free.
                                    reader.get_mut().until = None;
                                    let _ = reader.get_mut().stream.set_read_timeout(None);
                                    drop(slot.take());
                                    (json!({ "jsonrpc": "2.0", "id": id, "result": { "ok": true } }), false)
                                } else {
                                    (rpc_error(id, -32001, "authenticate first: auth {token}"), true)
                                }
                            } else {
                                let rx = client.send(method, params);
                                match rx.recv_timeout(std::time::Duration::from_secs(30)) {
                                    Ok(Ok(v)) => (json!({ "jsonrpc": "2.0", "id": id, "result": v }), false),
                                    Ok(Err(e)) => (rpc_error(id, -32000, &e), false),
                                    Err(_) => (rpc_error(id, -32002, "the app did not answer within 30 s"), false),
                                }
                            }
                        }
                    };
                    // `close` is only ever set before `auth`.
                    let place = if close { refuse(&mut slot, &lingering, limits) } else { None };
                    if writeln!(write, "{reply}").and_then(|_| write.flush()).is_err() {
                        break;
                    }
                    if close {
                        close_after_reply(reader.get_mut(), place, limits.linger);
                        break;
                    }
                }
            });
        }
    })?;
    Ok(Endpoint { port, token })
}

/// Longest line read before a connection has authenticated. The only acceptable request then is
/// `auth`, about a hundred bytes; a longer line is refused without buffering the rest of it.
#[cfg(not(target_arch = "wasm32"))]
const MAX_AUTH_LINE: usize = 4 * 1024;

/// What [`read_line_limited`] found.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, PartialEq, Eq)]
enum LineRead {
    /// A line, now in the buffer (newline and a trailing `\r` stripped).
    Line,
    /// End of stream.
    End,
    /// The line is longer than the limit; at most limit + 1 bytes of it were read.
    TooLong,
}

/// Read one `\n`-terminated line of at most `max` bytes into `buf`.
#[cfg(not(target_arch = "wasm32"))]
fn read_line_limited(reader: &mut impl std::io::BufRead, buf: &mut Vec<u8>, max: usize) -> std::io::Result<LineRead> {
    use std::io::{BufRead, Read};
    buf.clear();
    // `take` bounds what is buffered for one line: never more than `max` + 1 bytes.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    if reader.take(limit).read_until(b'\n', buf)? == 0 {
        return Ok(LineRead::End);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(LineRead::Line);
    }
    // No newline: either the limit was passed, or this is a last line before end of stream.
    Ok(if buf.len() > max { LineRead::TooLong } else { LineRead::Line })
}

#[cfg(not(target_arch = "wasm32"))]
fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(not(target_arch = "wasm32"))]
fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 128 random bits from the OS, hex-encoded.
#[cfg(not(target_arch = "wasm32"))]
fn random_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod transport_tests {
    /// A failed accept is followed by a wait before the next one, so a listener that keeps
    /// failing does not spin, and the connections around the failures still come through.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_failed_accept_waits_before_the_next() {
        let failed = || Err(std::io::Error::other("no file descriptors left"));
        let incoming = vec![failed(), Ok(1), failed(), failed(), Ok(2)];
        let mut waits = 0;
        let connections: Vec<i32> = super::accepted(incoming.into_iter(), || waits += 1).collect();
        assert_eq!(connections, [1, 2]);
        assert_eq!(waits, 3, "one wait per failed accept");
    }

    /// Before authenticating, a line that isn't JSON gets a parse error and closes the
    /// connection, as [`super::serve`] documents for any first request other than `auth`.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn malformed_input_before_auth_closes_the_connection() {
        use std::io::{BufRead, BufReader, ErrorKind, Write};
        let (_control, client) = crate::control::attach(&egui::Context::default());
        let ep = super::serve(client).unwrap();
        let s = std::net::TcpStream::connect(("127.0.0.1", ep.port)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        writeln!(s.try_clone().unwrap(), "not json").unwrap();
        let mut r = BufReader::new(s);
        let mut first = String::new();
        r.read_line(&mut first).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&first).unwrap();
        assert_eq!(reply["error"]["code"], -32700, "{first}");
        // Closed: end of stream (or a reset), not a connection left waiting for more input.
        let mut next = String::new();
        let closed = match r.read_line(&mut next) {
            Ok(n) => n == 0,
            Err(e) => matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted),
        };
        assert!(closed, "the connection stayed open: {next:?}");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn lines_are_read_up_to_the_limit() {
        use super::{LineRead, read_line_limited};
        let mut buf = Vec::new();
        let mut input: &[u8] = b"aaaaaaaaaa\nbb\r\ncc";
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
        assert_eq!(buf, b"aaaaaaaaaa", "a line exactly at the limit is accepted");
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
        assert_eq!(buf, b"bb", "CRLF is stripped");
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
        assert_eq!(buf, b"cc", "a last line without a newline is still read");
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::End, "then end of stream");
        let long = [b'x'; 100];
        let mut input: &[u8] = &long;
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::TooLong);
        assert_eq!(input.len(), 100 - 11, "at most limit + 1 bytes are read; the rest is left");
    }

    /// Before authenticating, a line longer than [`super::MAX_AUTH_LINE`] is refused as soon as
    /// the limit is passed (no newline needed) and the connection is closed.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn an_overlong_line_before_auth_is_refused() {
        use std::io::{BufRead, BufReader, ErrorKind, Write};
        let (_control, client) = crate::control::attach(&egui::Context::default());
        let ep = super::serve(client).unwrap();
        let s = std::net::TcpStream::connect(("127.0.0.1", ep.port)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        // Exactly one byte over the limit, without a newline: the server reads it all, so the
        // close below is a clean end of stream.
        s.try_clone().unwrap().write_all(&vec![b'x'; super::MAX_AUTH_LINE + 1]).unwrap();
        let mut r = BufReader::new(s);
        let mut first = String::new();
        r.read_line(&mut first).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&first).unwrap();
        assert_eq!(reply["error"]["code"], -32600, "{first}");
        let mut next = String::new();
        let closed = match r.read_line(&mut next) {
            Ok(n) => n == 0,
            Err(e) => matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted),
        };
        assert!(closed, "the connection stayed open: {next:?}");
    }

    /// Small limits for the tests below: a 300 ms deadline, two places to wait in.
    #[cfg(not(target_arch = "wasm32"))]
    const TEST_LIMITS: super::Limits = super::Limits {
        auth_deadline: std::time::Duration::from_millis(300),
        max_unauthenticated: 2,
        write_timeout: std::time::Duration::from_secs(5),
        linger: std::time::Duration::from_secs(1),
        max_lingering: 2,
    };

    /// The next line the server sends, as JSON, or `None` when it closed the connection instead.
    #[cfg(not(target_arch = "wasm32"))]
    fn next_line(r: &mut std::io::BufReader<std::net::TcpStream>) -> Option<serde_json::Value> {
        use std::io::BufRead;
        let mut line = String::new();
        match r.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(serde_json::from_str(&line).unwrap()),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn connect(port: u16) -> std::net::TcpStream {
        let s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        s
    }

    /// #1806: a connection that never authenticates is closed at the deadline, counted from its
    /// acceptance, and a client feeding a byte at a time cannot push that deadline back.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn an_unauthenticated_connection_is_closed_at_the_deadline() {
        use std::io::{BufReader, Write};
        let (_control, client) = crate::control::attach(&egui::Context::default());
        let ep = super::serve_with(client, TEST_LIMITS).unwrap();
        // Idle.
        let started = std::time::Instant::now();
        let mut r = BufReader::new(connect(ep.port));
        let reply = next_line(&mut r).expect("the server says why before closing");
        assert_eq!(reply["error"]["code"], -32004, "{reply}");
        let took = started.elapsed();
        assert!(took >= std::time::Duration::from_millis(250) && took < std::time::Duration::from_secs(3), "{took:?}");
        assert!(next_line(&mut r).is_none(), "then the connection is closed");
        // Dripping: a byte every 50 ms for as long as the server keeps the connection.
        let started = std::time::Instant::now();
        let s = connect(ep.port);
        let mut w = s.try_clone().unwrap();
        let drip = std::thread::spawn(move || {
            while w.write_all(b"x").is_ok() {
                std::thread::sleep(std::time::Duration::from_millis(50));
                if started.elapsed() > std::time::Duration::from_secs(3) {
                    break;
                }
            }
        });
        let mut r = BufReader::new(s);
        let reply = next_line(&mut r).expect("the server says why before closing");
        assert_eq!(reply["error"]["code"], -32004, "{reply}");
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(2), "the deadline is absolute: {took:?}");
        drop(r);
        drip.join().unwrap();
    }

    /// A client refused while the server still has its input unread gets the reason, not a
    /// reset: closing on top of unread input sends a reset, which can discard the reply before
    /// the client reads it (Linux does). Here a line far over the limit leaves most of itself
    /// unread when the server refuses it.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_client_refused_with_input_unread_gets_the_reason() {
        use std::io::{BufReader, Write};
        let (_control, client) = crate::control::attach(&egui::Context::default());
        let ep = super::serve_with(client, TEST_LIMITS).unwrap();
        let s = connect(ep.port);
        let mut w = s.try_clone().unwrap();
        w.write_all(&vec![b'x'; 16 * super::MAX_AUTH_LINE]).unwrap();
        // Let the server refuse it (and, before the fix, close) before reading the reply.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut r = BufReader::new(s);
        let reply = next_line(&mut r).expect("the server says why before closing");
        assert_eq!(reply["error"]["code"], -32600, "{reply}");
    }

    /// A refused connection gives back its place among those waiting to authenticate before it
    /// lingers, so lingering does not keep real clients out.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_lingering_connection_holds_no_place_to_authenticate() {
        use std::io::{BufReader, Write};
        // A linger far longer than the checks below take, even on a loaded machine.
        let limits = super::Limits { linger: std::time::Duration::from_secs(5), ..TEST_LIMITS };
        let (_control, client) = crate::control::attach(&egui::Context::default());
        let ep = super::serve_with(client, limits).unwrap();
        let auth = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "auth", "params": { "token": ep.token } });
        // Refused with most of its line unread, and left open: it lingers for `linger`.
        let refused = connect(ep.port);
        refused.try_clone().unwrap().write_all(&vec![b'x'; 16 * super::MAX_AUTH_LINE]).unwrap();
        let mut r = BufReader::new(refused.try_clone().unwrap());
        assert_eq!(next_line(&mut r).expect("refused with a reason")["error"]["code"], -32600);
        // Well within the linger, both places are free for new connections.
        let started = std::time::Instant::now();
        let others: Vec<_> = (0..limits.max_unauthenticated).map(|_| connect(ep.port)).collect();
        for s in &others {
            // A refused one has already been closed, so its write may fail; its reply says why.
            let _ = writeln!(s.try_clone().unwrap(), "{auth}");
            let reply = next_line(&mut BufReader::new(s.try_clone().unwrap())).expect("a reply");
            assert_eq!(reply["result"]["ok"], true, "admitted while the refused one lingers: {reply}");
        }
        assert!(started.elapsed() < limits.linger, "checked while it lingered: {:?}", started.elapsed());
        drop(refused);
    }

    /// Only [`super::Limits::max_lingering`] refused connections linger at once: one more still
    /// gets its reply, and is then closed at once instead of lingering.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn only_so_many_refused_connections_linger() {
        use std::io::{BufReader, Write};
        // A linger far longer than the checks below take, even on a loaded machine.
        let limits = super::Limits { linger: std::time::Duration::from_secs(5), max_lingering: 1, ..TEST_LIMITS };
        let (_control, client) = crate::control::attach(&egui::Context::default());
        let ep = super::serve_with(client, limits).unwrap();
        let bad = r#"{"jsonrpc": "2.0", "id": 1, "method": "page_count"}"#;
        // The first refused connection takes the only lingering place and keeps it.
        let first = connect(ep.port);
        writeln!(first.try_clone().unwrap(), "{bad}").unwrap();
        let mut r1 = BufReader::new(first.try_clone().unwrap());
        assert_eq!(next_line(&mut r1).expect("refused with a reason")["error"]["code"], -32001);
        // The second still gets its reason (it has nothing unread, so no reset)...
        let second = connect(ep.port);
        let mut w2 = second.try_clone().unwrap();
        writeln!(w2, "{bad}").unwrap();
        let mut r2 = BufReader::new(second.try_clone().unwrap());
        assert_eq!(next_line(&mut r2).expect("refused with a reason")["error"]["code"], -32001);
        // ...and is closed outright, not lingering: writing to it soon fails, where a lingering
        // connection would read and discard for the whole of `linger`.
        let started = std::time::Instant::now();
        let closed = loop {
            if w2.write_all(b"x").is_err() {
                break true;
            }
            if started.elapsed() > limits.linger {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(closed && started.elapsed() < limits.linger / 2, "it lingered: {:?}", started.elapsed());
        drop(first);
    }

    /// #1806: only so many connections may wait to authenticate at once. One more is refused
    /// on the spot; once a waiting one authenticates or times out, its place is free again.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn only_so_many_connections_may_wait_to_authenticate() {
        use std::io::{BufReader, Write};
        let (_control, client) = crate::control::attach(&egui::Context::default());
        let ep = super::serve_with(client, TEST_LIMITS).unwrap();
        let auth = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "auth", "params": { "token": ep.token } });
        let first = connect(ep.port);
        let second = connect(ep.port);
        // The kernel accepts a connection before the listener admits it: give the listener a
        // moment to admit both, well within their deadline.
        std::thread::sleep(std::time::Duration::from_millis(100));
        // The third finds both places taken.
        let mut r3 = BufReader::new(connect(ep.port));
        let reply = next_line(&mut r3).expect("refused with a reason");
        assert_eq!(reply["error"]["code"], -32003, "{reply}");
        assert!(next_line(&mut r3).is_none(), "and closed");
        // The second authenticates: its place is free, so a new connection is admitted.
        let mut r2 = BufReader::new(second.try_clone().unwrap());
        let mut w2 = second.try_clone().unwrap();
        writeln!(w2, "{auth}").unwrap();
        assert_eq!(next_line(&mut r2).unwrap()["result"]["ok"], true);
        let fourth = connect(ep.port);
        let mut r4 = BufReader::new(fourth.try_clone().unwrap());
        let mut w4 = fourth.try_clone().unwrap();
        writeln!(w4, "{auth}").unwrap();
        assert_eq!(next_line(&mut r4).unwrap()["result"]["ok"], true, "admitted and authenticated");
        // The first was closed at the deadline (its place freed on that way out too): after it,
        // another connection is admitted even though the first never authenticated.
        drop(first);
        std::thread::sleep(TEST_LIMITS.auth_deadline + std::time::Duration::from_millis(100));
        let fifth = connect(ep.port);
        let mut r5 = BufReader::new(fifth.try_clone().unwrap());
        let mut w5 = fifth.try_clone().unwrap();
        writeln!(w5, "{auth}").unwrap();
        assert_eq!(next_line(&mut r5).unwrap()["result"]["ok"], true);
    }

    /// After `auth` the limit no longer applies: a long line is read as before (here it isn't
    /// JSON, so it gets a parse error and the connection stays open).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn long_lines_after_auth_are_still_read() {
        use std::io::{BufRead, BufReader, Write};
        let (_control, client) = crate::control::attach(&egui::Context::default());
        let ep = super::serve(client).unwrap();
        let s = std::net::TcpStream::connect(("127.0.0.1", ep.port)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        let reply = |r: &mut BufReader<std::net::TcpStream>| {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            serde_json::from_str::<serde_json::Value>(&line).unwrap()
        };
        let auth = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "auth", "params": { "token": ep.token } });
        writeln!(w, "{auth}").unwrap();
        assert_eq!(reply(&mut r)["result"]["ok"], true);
        writeln!(w, "{}", "x".repeat(4 * super::MAX_AUTH_LINE)).unwrap();
        assert_eq!(reply(&mut r)["error"]["code"], -32700, "a long line after auth is parsed, not refused");
        writeln!(w, "y").unwrap();
        assert_eq!(reply(&mut r)["error"]["code"], -32700, "and the connection is still open");
    }
}
