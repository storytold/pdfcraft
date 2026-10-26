//! The controlling terminal (`/dev/tty`): its size, raw mode, and reads that give up after a
//! timeout. Opened separately from stdin and stdout, so it works with either redirected.
//!
//! Reads block on a thread of their own and reach callers through a channel, which they wait on
//! with a timeout. `poll(2)` can't wait on the terminal portably: on macOS it doesn't support
//! devices, and on `/dev/tty` it answers at once with `POLLNVAL`, so a read after it would block
//! until the next key.

use std::cell::OnceCell;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::time::Duration;

use rustix::termios::{OptionalActions, Termios, tcgetattr, tcgetwinsize, tcsetattr};

/// Reads the reader thread may queue before it waits for them to be taken: a bound on what a
/// terminal flooding its input can make `view` hold.
const QUEUED_READS: usize = 64;

pub(crate) struct Tty {
    file: File,
    /// What the reader thread has read. It starts with the first read and is the only reader
    /// from then on, so nothing typed is split between readers.
    input: OnceCell<Receiver<Input>>,
}

enum Input {
    Bytes(Vec<u8>),
    /// The terminal closed, or reading it failed: why.
    Closed(String),
}

/// The window's size in cells, and in pixels when the terminal reports them (0 otherwise).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowSize {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) xpixel: u16,
    pub(crate) ypixel: u16,
}

impl Tty {
    pub(crate) fn open() -> Result<Self, String> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map(|file| Self { file, input: OnceCell::new() })
            .map_err(|e| format!("no terminal to draw on (/dev/tty: {e})"))
    }

    /// `None` when the terminal doesn't say, or says it has no cells.
    pub(crate) fn size(&self) -> Option<WindowSize> {
        let w = tcgetwinsize(&self.file).ok()?;
        (w.ws_col > 0 && w.ws_row > 0).then_some(WindowSize { cols: w.ws_col, rows: w.ws_row, xpixel: w.ws_xpixel, ypixel: w.ws_ypixel })
    }

    /// Switch to raw mode (no echo, no line editing, bytes as they come) until the guard drops.
    pub(crate) fn raw(&self) -> Result<RawMode<'_>, String> {
        let saved = tcgetattr(&self.file).map_err(|e| format!("terminal settings: {e}"))?;
        let mut raw = saved.clone();
        raw.make_raw();
        tcsetattr(&self.file, OptionalActions::Now, &raw).map_err(|e| format!("terminal settings: {e}"))?;
        Ok(RawMode { tty: self, saved })
    }

    /// The bytes that arrive within `timeout`, or `None` when none do. A terminal that has gone
    /// away is an error, never an endless run of empty reads for a caller to spin on.
    pub(crate) fn read_timeout(&self, timeout: Duration) -> Result<Option<Vec<u8>>, String> {
        match self.input()?.recv_timeout(timeout) {
            Ok(Input::Bytes(bytes)) => Ok(Some(bytes)),
            Ok(Input::Closed(why)) => Err(why),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err("the terminal closed".into()),
        }
    }

    /// The reader thread's channel, starting the thread the first time.
    fn input(&self) -> Result<&Receiver<Input>, String> {
        if let Some(input) = self.input.get() {
            return Ok(input);
        }
        let file = self.file.try_clone().map_err(|e| format!("reading the terminal: {e}"))?;
        let (sender, receiver) = sync_channel(QUEUED_READS);
        // A thread still blocked in a read when `view` finishes ends with the process.
        std::thread::Builder::new()
            .name("view-terminal-input".into())
            .spawn(move || read_until_closed(file, &sender))
            .map_err(|e| format!("reading the terminal: {e}"))?;
        Ok(self.input.get_or_init(|| receiver))
    }

    pub(crate) fn write_all(&self, bytes: &[u8]) -> Result<(), String> {
        let mut file = &self.file;
        file.write_all(bytes).and_then(|()| file.flush()).map_err(|e| format!("writing to the terminal: {e}"))
    }
}

/// Pass on what `file` reads until it closes or fails, or nobody listens any more.
fn read_until_closed(mut file: File, sender: &SyncSender<Input>) {
    let mut buf = [0u8; 256];
    loop {
        let input = match file.read(&mut buf) {
            Ok(0) => Input::Closed("the terminal closed".into()),
            Ok(n) => Input::Bytes(buf.get(..n).unwrap_or_default().to_vec()),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => Input::Closed(format!("reading the terminal: {e}")),
        };
        let closed = matches!(input, Input::Closed(_));
        if sender.send(input).is_err() || closed {
            return;
        }
    }
}

/// Raw mode, restored when this drops: on return, on an error through `?` and while a panic
/// unwinds.
pub(crate) struct RawMode<'a> {
    tty: &'a Tty,
    saved: Termios,
}

impl Drop for RawMode<'_> {
    fn drop(&mut self) {
        let _ = tcsetattr(&self.tty.file, OptionalActions::Now, &self.saved);
    }
}
