//! Single-instance handoff (desktop only, #367).
//!
//! Windows Explorer launches one PdfCraft per selected file, even for the Combine verb
//! registered with `MultiSelectModel = Player`: each process arrives with `--combine` and a
//! single file, so without a handoff every file would open its own window with its own
//! one-file Combine list. Instead the first instance (the primary) listens on loopback and
//! later launches forward their request and exit, so everything lands in one window.
//!
//! The election is race-free: claiming the lock file is atomic, so five simultaneous
//! launches (an Explorer multi-select with nothing running yet) elect exactly one primary
//! however they interleave. A lock naming a dead port is a crashed primary and is taken
//! over; a lock with no port yet is a starter still binding, and waiters hold on briefly.
//!
//! This is deliberately narrower than the opt-in UI control channel (AGENTS.md §3): it can
//! neither run commands nor read anything back. It only stages file paths, exactly as
//! launching the app with those arguments would (a local process could do that itself, so a
//! handoff grants nothing new). The lock file lives in the settings folder — per user, and
//! per portable copy — the listener binds 127.0.0.1 on an OS-assigned port, every number that
//! sizes anything is capped, and anything malformed is ignored. When anything at all goes
//! wrong the launch behaves as if the handoff did not exist: the files open in the new
//! window instead of being lost. A `--control` launch never forwards (its driver expects that
//! exact process to own the window) and never listens.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// What a later launch asks the running instance to stage: the files in command-line order
/// and which staging mode (`--combine`, `--create-images`, or plain opens).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HandoffRequest {
    /// Paths in the order given on the command line.
    #[serde(default)]
    pub files: Vec<String>,
    /// Stage them in the Combine files tab.
    #[serde(default)]
    pub combine: bool,
    /// Stage them in the image-import chooser.
    #[serde(default)]
    pub create_images: bool,
}

impl HandoffRequest {
    /// A bare launch (no files, no staging flag) opens its own window as before; only a
    /// launch carrying something is worth forwarding.
    pub fn carries_files(&self) -> bool {
        !self.files.is_empty() || self.combine || self.create_images
    }
}

/// What `claim` decided for this launch.
pub enum Claim {
    /// Run normally. The listener (when present) takes later launches' files; without one
    /// (an unwritable settings folder) this instance simply stands alone.
    Primary(Option<Handoff>),
    /// The running instance accepted the request: exit quietly.
    Forwarded,
}

/// Decide this launch's role: forward to the running instance when there is one and this
/// launch carries files, otherwise run (and listen for later launches). Never fails the
/// launch: the worst case is a standalone window, today's behaviour.
pub fn claim(dir: &Path, req: &HandoffRequest) -> Claim {
    if req.carries_files() && forward(dir, req) {
        return Claim::Forwarded;
    }
    Claim::Primary(become_primary(dir))
}

/// The primary side: a loopback listener plus the lock file advertising it.
pub struct Handoff {
    listener: TcpListener,
    lock: PathBuf,
    port: u16,
    pid: u32,
    pending: VecDeque<Pending>,
}

/// A connection that has not sent its whole line yet (kept across frames; the listener and
/// every connection are non-blocking, so polling never stalls the UI).
struct Pending {
    stream: TcpStream,
    buf: Vec<u8>,
}

/// A request is one JSON object per line; anything else on the connection is ignored.
const LOCK_NAME: &str = "single-instance.json";
/// The most files one handoff may stage (memory stays bounded by the line cap below).
const MAX_FILES: usize = 4096;
/// The most characters one staged path may hold.
const MAX_PATH_CHARS: usize = 32768;
/// The most bytes one connection may send before it is dropped.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Connections kept at once; beyond that the oldest waiting one is dropped.
const MAX_PENDING: usize = 16;
/// How long a new launch waits for the primary per attempt.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long a new launch waits for the primary's acknowledgement.
const ACK_TIMEOUT: Duration = Duration::from_secs(5);
/// Election rounds while a starter publishes its port (one round is fast when a live
/// primary answers; the whole wait only happens for a crashed one).
const SLOW_ATTEMPTS: u32 = 10;
/// Pause between election rounds.
const SLOW_WAIT: Duration = Duration::from_millis(100);
/// A lock older than this, naming a port nobody answers on, is a crashed primary: take
/// over at once instead of waiting out the election.
const STALE_AFTER: Duration = Duration::from_secs(10);
/// What the primary answers a well-formed request with.
const ACK: &str = "{\"ok\":true}\n";

/// What the lock file holds: where the primary listens.
#[derive(serde::Serialize, serde::Deserialize)]
struct Lock {
    port: u16,
    pid: u32,
}

impl Handoff {
    /// Take requests that arrived whole since the last frame, oldest first.
    pub fn poll(&mut self) -> Vec<HandoffRequest> {
        let mut out = Vec::new();
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if stream.set_nonblocking(true).is_err() {
                        continue;
                    }
                    if self.pending.len() >= MAX_PENDING {
                        let _ = self.pending.pop_front();
                    }
                    self.pending.push_back(Pending { stream, buf: Vec::new() });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        let mut i = 0;
        while i < self.pending.len() {
            if self.read_one(i, &mut out) {
                let _ = self.pending.remove(i);
            } else {
                i += 1;
            }
        }
        out
    }

    /// Read what is available on connection `i`; true when it is finished (answered or
    /// hopeless) and can be dropped.
    fn read_one(&mut self, i: usize, out: &mut Vec<HandoffRequest>) -> bool {
        let Some(conn) = self.pending.get_mut(i) else { return true };
        let mut chunk = [0u8; 8192];
        loop {
            match conn.stream.read(&mut chunk) {
                Ok(0) => return true, // closed, with or without a line
                Ok(n) => {
                    let Some(bytes) = chunk.get(..n) else { return true };
                    if conn.buf.len().saturating_add(bytes.len()) > MAX_LINE_BYTES {
                        return true;
                    }
                    conn.buf.extend_from_slice(bytes);
                    if conn.buf.contains(&b'\n') {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return false,
                Err(_) => return true,
            }
        }
        let Some(conn) = self.pending.get_mut(i) else { return true };
        let line = std::mem::take(&mut conn.buf);
        if let Some(req) = parse(&line) {
            out.push(req);
            let _ = conn.stream.write_all(ACK.as_bytes());
        }
        true
    }
}

impl Drop for Handoff {
    /// Take the lock file away, but only while it still names us: a successor's lock (after
    /// a crash and a takeover) is left alone.
    fn drop(&mut self) {
        if read_lock(&self.lock).is_some_and(|l| l.port == self.port && l.pid == self.pid) {
            let _ = std::fs::remove_file(&self.lock);
        }
    }
}

/// A line is a request when it parses and stays within the caps (counts, not bytes: the
/// line itself is already capped, so memory stays bounded). Anything else is ignored, and
/// the sender — getting no acknowledgement — opens its files itself instead of losing them.
fn parse(line: &[u8]) -> Option<HandoffRequest> {
    let req: HandoffRequest = serde_json::from_slice(line).ok()?;
    (req.files.len() <= MAX_FILES && req.files.iter().all(|f| f.chars().count() <= MAX_PATH_CHARS)).then_some(req)
}

/// Hand the request to the primary named by the lock file. True once the primary
/// acknowledged it. A missing lock is nobody home; a lock with no port yet is a starter
/// still binding (wait for its port); a lock naming a dead port is either a racing
/// starter (wait briefly) or a crashed primary (taken over, at once when long stale).
fn forward(dir: &Path, req: &HandoffRequest) -> bool {
    if let Some(lock) = read_lock(&dir.join(LOCK_NAME))
        && lock.port != 0
        && try_forward(lock.port, req)
    {
        return true;
    }
    for _ in 0..SLOW_ATTEMPTS {
        if create_placeholder(dir) {
            return false; // atomic win: we publish our port in become_primary
        }
        match read_lock_raw(&dir.join(LOCK_NAME)) {
            Some(lock) if lock.port != 0 => {
                if try_forward(lock.port, req) {
                    return true;
                }
                if lock_age(dir).is_some_and(|age| age > STALE_AFTER) {
                    return false;
                }
            }
            Some(_) => {}         // a starter is mid-flight; wait for its port
            None => return false, // missing or garbage: take it
        }
        std::thread::sleep(SLOW_WAIT);
    }
    false
}

/// Send one request and wait for the acknowledgement.
fn try_forward(port: u16, req: &HandoffRequest) -> bool {
    let Ok(line) = serde_json::to_string(req) else { return false };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(stream) = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) else { return false };
    if stream.set_write_timeout(Some(ACK_TIMEOUT)).is_err() || stream.set_read_timeout(Some(ACK_TIMEOUT)).is_err() {
        return false;
    }
    let Ok(mut read) = stream.try_clone() else { return false };
    let mut write = stream;
    if write.write_all(line.as_bytes()).is_err() || write.write_all(b"\n").is_err() || write.flush().is_err() {
        return false;
    }
    // The acknowledgement is fixed and tiny; anything else (or silence) means the handoff
    // did not happen, and the files open here instead.
    let mut ack = [0u8; 64];
    let mut n = 0;
    loop {
        if n >= ack.len() {
            return false;
        }
        let Some(slot) = ack.get_mut(n..) else { return false };
        match read.read(slot) {
            Ok(0) => return false,
            Ok(m) => {
                n = n.saturating_add(m);
                if ack.get(..n).is_some_and(|seen| seen.contains(&b'\n')) {
                    break;
                }
            }
            Err(_) => return false,
        }
    }
    ack.get(..n).is_some_and(|seen| seen == ACK.as_bytes())
}

/// Become the primary: listen, then advertise the listener. `None` (a standalone window)
/// when the settings folder can't be used.
fn become_primary(dir: &Path) -> Option<Handoff> {
    let _ = std::fs::create_dir_all(dir);
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(_) => {
            remove_own_placeholder(dir);
            return None;
        }
    };
    let port = listener.local_addr().ok()?.port();
    if port == 0 || listener.set_nonblocking(true).is_err() {
        remove_own_placeholder(dir);
        return None;
    }
    if write_lock(dir, port).is_none() {
        remove_own_placeholder(dir);
        return None;
    }
    Some(Handoff { listener, lock: dir.join(LOCK_NAME), port, pid: std::process::id(), pending: VecDeque::new() })
}

fn read_lock(path: &Path) -> Option<Lock> {
    read_lock_raw(path).filter(|l| l.port != 0)
}

/// The lock file as written, placeholders included.
fn read_lock_raw(path: &Path) -> Option<Lock> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// How old the lock file is (a racing starter's is seconds fresh; a crashed primary's
/// keeps ageing). `None` when the file is gone or the clock can't say.
fn lock_age(dir: &Path) -> Option<Duration> {
    std::fs::metadata(dir.join(LOCK_NAME)).ok()?.modified().ok()?.elapsed().ok()
}

/// Atomically claim the election: exactly one racing launch creates the lock, as a
/// port-less placeholder until `become_primary` publishes the port. Losers see the
/// placeholder (wait) or the published port (forward).
fn create_placeholder(dir: &Path) -> bool {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(LOCK_NAME);
    let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    if let Ok(text) = serde_json::to_string(&Lock { port: 0, pid: std::process::id() }) {
        let _ = file.write_all(text.as_bytes());
    }
    true
}

/// Drop a placeholder this process claimed but never published (binding failed): it must
/// not trap the next launch into waiting. Another launch's lock is left alone.
fn remove_own_placeholder(dir: &Path) {
    let path = dir.join(LOCK_NAME);
    if read_lock_raw(&path).is_some_and(|l| l.port == 0 && l.pid == std::process::id()) {
        let _ = std::fs::remove_file(&path);
    }
}

/// Publish the lock through a uniquely named temporary file and a rename, so a racing
/// launch reads the old lock or the new one, never half of one.
fn write_lock(dir: &Path, port: u16) -> Option<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let text = serde_json::to_string(&Lock { port, pid: std::process::id() }).ok()?;
    let tmp = dir.join(format!(".{LOCK_NAME}.{}.{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&tmp, text.as_bytes()).ok()?;
    if std::fs::rename(&tmp, dir.join(LOCK_NAME)).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder per call: tests run in parallel and each primary owns its lock.
    fn scratch() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("pdfcraft-handoff-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(files: &[&str], combine: bool) -> HandoffRequest {
        HandoffRequest { files: files.iter().map(|s| s.to_string()).collect(), combine, create_images: false }
    }

    /// Poll a primary on a worker thread until `want` requests arrived: a forward only
    /// completes once the primary answers, so the primary must pump while forwarding.
    fn background(mut primary: Handoff, want: usize) -> std::sync::mpsc::Receiver<(Handoff, Vec<HandoffRequest>)> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut got = Vec::new();
            for _ in 0..1000 {
                got.extend(primary.poll());
                if got.len() >= want {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = tx.send((primary, got));
        });
        rx
    }

    fn receive(rx: std::sync::mpsc::Receiver<(Handoff, Vec<HandoffRequest>)>, want: usize) -> (Handoff, Vec<HandoffRequest>) {
        let (primary, got) = rx.recv_timeout(Duration::from_secs(15)).unwrap();
        assert_eq!(got.len(), want, "every forwarded launch arrived");
        (primary, got)
    }

    #[test]
    fn files_reach_the_primary_in_order() {
        let dir = scratch();
        // The first launch becomes the primary and stages its own files itself.
        let Claim::Primary(Some(primary)) = claim(&dir, &request(&["b.pdf", "a.pdf"], true)) else {
            panic!("the first launch becomes the primary");
        };
        let rx = background(primary, 2);
        assert!(matches!(claim(&dir, &request(&["c.pdf"], false)), Claim::Forwarded), "a later launch forwards");
        assert!(matches!(claim(&dir, &request(&["d.pdf"], true)), Claim::Forwarded));
        let (mut primary, got) = receive(rx, 2);
        assert_eq!(got[0].files, ["c.pdf"]);
        assert!(!got[0].combine);
        assert_eq!(got[1].files, ["d.pdf"]);
        assert!(got[1].combine);
        assert!(primary.poll().is_empty(), "nothing arrives twice");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn simultaneous_starts_elect_exactly_one_primary() {
        // An Explorer multi-select with nothing running: every launch races from scratch.
        let dir = std::sync::Arc::new(scratch());
        let mut handles = Vec::new();
        for i in 0..5 {
            let dir = dir.clone();
            handles.push(std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(i * 13));
                let name = format!("f{i}.pdf");
                let req = HandoffRequest { files: vec![name], combine: true, create_images: false };
                match claim(&dir, &req) {
                    Claim::Forwarded => "forwarded".to_string(),
                    Claim::Primary(Some(mut primary)) => {
                        // Act as the running app: pump until every file arrived.
                        let mut got = 0;
                        for _ in 0..400 {
                            got += primary.poll().len();
                            if got >= 4 {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        format!("primary:{got}")
                    }
                    Claim::Primary(None) => "standalone".to_string(),
                }
            }));
        }
        let mut roles: Vec<String> = handles.into_iter().map(|t| t.join().unwrap()).collect();
        roles.sort();
        assert_eq!(roles.iter().filter(|r| r.starts_with("primary")).count(), 1, "{roles:?}");
        assert_eq!(roles.iter().filter(|r| *r == "forwarded").count(), 4, "{roles:?}");
        assert!(roles.contains(&"primary:4".to_string()), "{roles:?}");
        let _ = std::fs::remove_dir_all(&*dir);
    }

    #[test]
    fn a_bare_launch_never_forwards_but_still_listens() {
        let dir = scratch();
        let Claim::Primary(Some(mut primary)) = claim(&dir, &request(&["a.pdf"], false)) else {
            panic!("the first launch becomes the primary");
        };
        // A bare launch (no files, no flags) always opens its own window…
        let Claim::Primary(Some(bare)) = claim(&dir, &HandoffRequest::default()) else {
            panic!("a bare launch never forwards");
        };
        assert!(primary.poll().is_empty(), "nothing was handed anywhere");
        // …while still listening, so later files find *a* running instance (the newest one).
        let rx = background(bare, 1);
        assert!(matches!(claim(&dir, &request(&["b.pdf"], false)), Claim::Forwarded));
        let (_, got) = receive(rx, 1);
        assert_eq!(got[0].files, ["b.pdf"]);
        assert!(primary.poll().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_and_dead_locks_are_taken_over() {
        let dir = scratch();
        std::fs::write(dir.join(LOCK_NAME), b"not json").unwrap();
        let Claim::Primary(first) = claim(&dir, &HandoffRequest::default()) else {
            panic!("unreachable: bare launches never forward");
        };
        assert!(first.is_some(), "a garbage lock does not stop the primary");
        drop(first);
        // A lock naming a port nobody listens on (a crashed primary): claimed after waiting
        // for the race it almost certainly is not.
        let dead = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = dead.local_addr().unwrap().port();
        drop(dead);
        std::fs::write(dir.join(LOCK_NAME), serde_json::to_string(&Lock { port, pid: 1 }).unwrap()).unwrap();
        let Claim::Primary(second) = claim(&dir, &request(&["a.pdf"], false)) else {
            panic!("files never forward into a dead port");
        };
        assert!(second.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_garbled_line_is_ignored_and_its_sender_keeps_its_files() {
        let dir = scratch();
        let Claim::Primary(Some(mut primary)) = claim(&dir, &request(&["a.pdf"], false)) else {
            panic!("the first launch becomes the primary");
        };
        let port = primary.port;
        let mut raw = TcpStream::connect(("127.0.0.1", port)).unwrap();
        raw.write_all(b"{oops\n").unwrap();
        drop(raw);
        assert!(primary.poll().is_empty(), "nothing parses, nothing stages");
        // No acknowledgement went out, so a real second launch would open its files itself.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn absurd_requests_are_rejected() {
        assert!(parse(b"{\"files\":[],\"combine\":false,\"create_images\":false}").is_some());
        assert!(parse(b"nope").is_none());
        let many = HandoffRequest { files: vec!["a.pdf".to_string(); MAX_FILES + 1], combine: false, create_images: false };
        assert!(parse(&serde_json::to_vec(&many).unwrap()).is_none(), "too many files");
        let long = HandoffRequest { files: vec!["x".repeat(MAX_PATH_CHARS + 1)], combine: false, create_images: false };
        assert!(parse(&serde_json::to_vec(&long).unwrap()).is_none(), "too long a path");
    }

    #[test]
    fn dropping_the_primary_releases_a_lock_it_still_owns() {
        let dir = scratch();
        let handoff = become_primary(&dir).unwrap();
        assert!(dir.join(LOCK_NAME).is_file());
        drop(handoff);
        assert!(!dir.join(LOCK_NAME).exists(), "no stale lock left behind");
        // …while a successor's lock (after a takeover) is left alone.
        let handoff = become_primary(&dir).unwrap();
        std::fs::write(dir.join(LOCK_NAME), serde_json::to_string(&Lock { port: 1, pid: 1 }).unwrap()).unwrap();
        drop(handoff);
        assert!(dir.join(LOCK_NAME).is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
