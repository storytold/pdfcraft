//! Pages handed to a terminal on this machine through temporary files (`t=t` in the kitty
//! graphics protocol): a page's pixels go into a file of their own, which the terminal reads and
//! then deletes, and only the file's path goes through the terminal stream (and tmux). Neither
//! side makes or decodes a PNG: kitty takes a page in several times faster than as a PNG in the
//! stream, and tmux has next to nothing to pass on.
//!
//! Whether the terminal reads this machine's files is found out by trying ([`TempFiles::start`]):
//! a one-pixel image sent through a file, which the terminal deletes once it has read it. Until
//! then, and for good if it never does (a terminal across SSH, or one without temporary files),
//! pages go as PNGs in the stream.
//!
//! The protocol has the terminal delete only files in a temporary directory with
//! `tty-graphics-protocol` in their path. The files go in a directory of this user's own inside
//! the temporary directory, `tty-graphics-protocol-pdfcraft-<uid>`: no one else can create,
//! replace or read anything in it, and looking for files left behind lists only it (a shared
//! temporary directory can hold hundreds of thousands of entries, which take seconds to list).
//! Each file is created new, never an existing file nor through a symlink. A file the terminal
//! leaves is removed after [`STALE`], when the pager ends, and, when the run was killed first,
//! by a later run.
//!
//! The files are made, looked at and removed on a thread of their own ([`Files`]): a filesystem
//! that stalls (a hung network mount, say) stalls that thread, never the pager's keys.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

use pdfcraft_engine::export::unpremultiply;
use pdfcraft_render::RenderedPage;

use super::kitty::{self, Wrap};

/// The directory's name before the user id: the protocol's marker, then whose files they are.
const DIR_PREFIX: &str = "tty-graphics-protocol-pdfcraft-";
/// How long the terminal has to take the probe. Pages go in the stream meanwhile, and for good
/// after it.
const PROBE_WAIT: Duration = Duration::from_secs(2);
/// How long a file waits for the terminal before it is removed. The terminal takes one in
/// milliseconds; one it hasn't taken by now never will (it doesn't take files after all, or tmux
/// threw the escape code away), and pages go in the stream from then on.
const STALE: Duration = Duration::from_secs(5);
/// How old a file of a run that is gone must be before another run removes it.
const LEFT_BEHIND: Duration = Duration::from_secs(60);
/// The most entries looked through for files left behind.
const MAX_SWEPT: usize = 10_000;
/// The most files waiting for the terminal at once; pages go in the stream until it catches up.
const MAX_WAITING: usize = 8;
/// Bytes converted to straight alpha and written at a time: whole pixels.
const WRITE_CHUNK: usize = 1 << 18;
/// How long the pager waits for the files' thread to answer before it gives files up for the
/// run. Writing the largest page takes tens of milliseconds.
const STALL: Duration = Duration::from_secs(1);

/// The temporary files, for the pager: [`TempFiles`] on a thread of its own. Each call waits at
/// most [`STALL`] for the thread's answer. After one that doesn't come, files are given up for
/// the rest of the run, and the pages whose files the terminal hadn't taken yet count as lost.
/// When this drops, the thread removes the files, and this waits for that at most [`STALL`],
/// stalled or not: a thread that was only slow is done by then. The pager keeps a stalled one to
/// its end, after the terminal is put back. A run that ends first leaves them to a later run.
pub(crate) struct Files {
    jobs: Sender<Job>,
    answers: Receiver<Answer>,
    stall: Duration,
    /// The images whose files the terminal hadn't taken at the last look, and the one being
    /// written: the ones lost if the thread stalls.
    waiting: Vec<u32>,
    stalled: bool,
}

enum Job {
    Ready(Instant),
    Write { id: u32, page: RenderedPage, now: Instant },
    Check(Instant),
    Forget,
    Stop,
}

enum Answer {
    /// The probe's escape codes, or `None` when the files can't be.
    Started(Option<Vec<u8>>),
    Ready(bool),
    /// The page comes back, for the stream when its file failed.
    Written(Result<PathBuf, String>, RenderedPage),
    Checked {
        lost: Vec<u32>,
        waiting: Vec<u32>,
    },
    Stopped,
}

/// What became of a page given to [`Files::write`].
pub(crate) enum Written {
    File(PathBuf),
    /// It couldn't be written, and why; the page comes back, to go in the stream.
    Failed(String, RenderedPage),
    /// The thread didn't answer in time (or is gone): the page went with it, and must be
    /// rendered again.
    Stalled,
}

impl Files {
    /// Start trying, on a thread of its own ([`TempFiles::start`]): the files, and the probe's
    /// escape codes to write before any page. `None` when there's no safe place for them, or no
    /// answer in time.
    pub(crate) fn start(probe_id: u32, wrap: Wrap) -> Option<(Self, Vec<u8>)> {
        Self::spawn(STALL, move || TempFiles::start(probe_id, wrap, Instant::now()))
    }

    fn spawn(stall: Duration, start: impl FnOnce() -> Option<(TempFiles, Vec<u8>)> + Send + 'static) -> Option<(Self, Vec<u8>)> {
        let (jobs, inbox) = mpsc::channel();
        let (outbox, answers) = mpsc::channel();
        std::thread::Builder::new().name("view-files".into()).spawn(move || serve(start, &inbox, &outbox)).ok()?;
        let mut files = Self { jobs, answers, stall, waiting: Vec::new(), stalled: false };
        if let Some(Answer::Started(Some(probe))) = files.answer() {
            return Some((files, probe));
        }
        // No files, and no second wait: a start that ends late removes its probe by itself, while
        // the pager goes on.
        files.stall = Duration::ZERO;
        None
    }

    /// Whether the next page can go through a file ([`TempFiles::ready`]).
    pub(crate) fn ready(&mut self, now: Instant) -> bool {
        matches!(self.ask(Job::Ready(now)), Some(Answer::Ready(true)))
    }

    /// Write `page` to a new file for image `id` ([`TempFiles::write`]).
    pub(crate) fn write(&mut self, id: u32, page: RenderedPage, now: Instant) -> Written {
        self.waiting.push(id);
        match self.ask(Job::Write { id, page, now }) {
            Some(Answer::Written(Ok(path), _)) => Written::File(path),
            Some(Answer::Written(Err(e), page)) => {
                self.waiting.retain(|w| *w != id);
                Written::Failed(e, page)
            }
            _ => Written::Stalled,
        }
    }

    /// The images whose files the terminal never took ([`TempFiles::check`]). Once the thread has
    /// stalled: once, every image whose file it hadn't taken yet, as far as is known.
    pub(crate) fn check(&mut self, now: Instant) -> Vec<u32> {
        match self.ask(Job::Check(now)) {
            Some(Answer::Checked { lost, waiting }) => {
                self.waiting = waiting;
                lost
            }
            _ => std::mem::take(&mut self.waiting),
        }
    }

    /// Remove the files waiting for the terminal without giving files up: it can't have them
    /// (tmux threw their escape codes away), and their pages will be sent again.
    pub(crate) fn forget(&mut self) {
        if !self.stalled {
            let _ = self.jobs.send(Job::Forget);
        }
        self.waiting.clear();
    }

    /// Whether the thread stopped answering: files are over for this run.
    pub(crate) fn stalled(&self) -> bool {
        self.stalled
    }

    /// The thread's answer to `job`; `None`, for good, once it doesn't come in time.
    fn ask(&mut self, job: Job) -> Option<Answer> {
        if self.stalled || self.jobs.send(job).is_err() {
            self.stalled = true;
            return None;
        }
        self.answer()
    }

    fn answer(&mut self) -> Option<Answer> {
        let answer = self.answers.recv_timeout(self.stall).ok();
        self.stalled |= answer.is_none();
        answer
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        if self.jobs.send(Job::Stop).is_err() {
            return;
        }
        // Stopped, once the files are removed: after the answer to a job that stalled, if any.
        let Some(deadline) = Instant::now().checked_add(self.stall) else { return };
        while let Ok(answer) = self.answers.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            if matches!(answer, Answer::Stopped) {
                break;
            }
        }
    }
}

/// The files' thread: start them, then do the pager's jobs until it stops or goes.
fn serve(start: impl FnOnce() -> Option<(TempFiles, Vec<u8>)>, jobs: &Receiver<Job>, answers: &Sender<Answer>) {
    let Some((mut files, probe)) = start() else {
        let _ = answers.send(Answer::Started(None));
        return;
    };
    if answers.send(Answer::Started(Some(probe))).is_ok() {
        while let Ok(job) = jobs.recv() {
            let answer = match job {
                Job::Ready(now) => Answer::Ready(files.ready(now)),
                Job::Write { id, page, now } => Answer::Written(files.write(id, page.width, page.height, &page.rgba, now), page),
                Job::Check(now) => {
                    let lost = files.check(now);
                    Answer::Checked { lost, waiting: files.waiting.iter().map(|w| w.id).collect() }
                }
                Job::Forget => {
                    files.forget();
                    continue;
                }
                Job::Stop => break,
            };
            if answers.send(answer).is_err() {
                break;
            }
        }
    }
    drop(files);
    let _ = answers.send(Answer::Stopped);
}

enum Probe {
    /// Sent; the terminal hasn't taken it yet.
    Waiting { path: PathBuf, since: Instant },
    /// The terminal reads files from here.
    Taken,
    /// It didn't in time, or a file couldn't be written: pages go in the stream.
    Refused,
}

/// A page's file, written and not taken yet.
struct Waiting {
    path: PathBuf,
    id: u32,
    since: Instant,
}

struct TempFiles {
    /// This user's directory.
    dir: PathBuf,
    /// This process: the first part of its files' names, `<run>-<n>`.
    run: u32,
    next: u64,
    probe: Probe,
    waiting: Vec<Waiting>,
}

impl TempFiles {
    /// Start trying, in the temporary directory: these files, and the escape codes of the probe
    /// (image `probe_id`, deleted again at once) to write before any page. `None` when there's no
    /// safe place for the files or the probe can't be written.
    fn start(probe_id: u32, wrap: Wrap, now: Instant) -> Option<(Self, Vec<u8>)> {
        let uid = rustix::process::getuid().as_raw();
        Self::start_in(&temp_dir()?, uid, std::process::id(), probe_id, wrap, now)
    }

    fn start_in(parent: &Path, uid: u32, run: u32, probe_id: u32, wrap: Wrap, now: Instant) -> Option<(Self, Vec<u8>)> {
        let dir = own_dir(parent, uid)?;
        sweep(&dir, uid, SystemTime::now());
        let mut files = Self { dir, run, next: 0, probe: Probe::Refused, waiting: Vec::new() };
        // One opaque white pixel.
        let path = files.create(1, 1, &[255; 4]).ok()?;
        let mut out = Vec::new();
        kitty::transmit_file(&mut out, probe_id, &path, 1, 1, wrap);
        kitty::delete(&mut out, probe_id, wrap);
        files.probe = Probe::Waiting { path, since: now };
        Some((files, out))
    }

    /// Whether the next page can go through a file: the terminal took the probe, and isn't
    /// behind with the files since.
    fn ready(&mut self, now: Instant) -> bool {
        self.settle(now);
        matches!(self.probe, Probe::Taken) && self.waiting.len() < MAX_WAITING
    }

    /// Write `width` × `height` premultiplied RGBA pixels, as straight RGBA, to a new file for
    /// image `id`, and say where. After a failure, pages go in the stream.
    fn write(&mut self, id: u32, width: u32, height: u32, premultiplied: &[u8], now: Instant) -> Result<PathBuf, String> {
        match self.create(width, height, premultiplied) {
            Ok(path) => {
                self.waiting.push(Waiting { path: path.clone(), id, since: now });
                Ok(path)
            }
            Err(e) => {
                self.probe = Probe::Refused;
                Err(e)
            }
        }
    }

    /// Forget the files the terminal has taken, and remove those it hasn't after [`STALE`]: the
    /// ids of their images, which never arrived. Pages go in the stream after one such file,
    /// whatever the probe said: something else may have removed the probe.
    fn check(&mut self, now: Instant) -> Vec<u32> {
        self.settle(now);
        let mut lost = Vec::new();
        self.waiting.retain(|w| {
            if !present(&w.path) {
                return false;
            }
            if now.saturating_duration_since(w.since) < STALE {
                return true;
            }
            let _ = std::fs::remove_file(&w.path);
            lost.push(w.id);
            false
        });
        if !lost.is_empty() {
            self.probe = Probe::Refused;
        }
        lost
    }

    /// Remove the files waiting, without giving files up ([`Files::forget`]).
    fn forget(&mut self) {
        for w in self.waiting.drain(..) {
            let _ = std::fs::remove_file(&w.path);
        }
    }

    /// Settle the probe once the terminal has taken it, or the wait is over.
    fn settle(&mut self, now: Instant) {
        if let Probe::Waiting { path, since } = &self.probe {
            if !present(path) {
                self.probe = Probe::Taken;
            } else if now.saturating_duration_since(*since) >= PROBE_WAIT {
                let _ = std::fs::remove_file(path);
                self.probe = Probe::Refused;
            }
        }
    }

    /// A new file holding the pixels, or why not; nothing is left behind on failure.
    fn create(&mut self, width: u32, height: u32, premultiplied: &[u8]) -> Result<PathBuf, String> {
        let expected = u64::from(width).checked_mul(u64::from(height)).and_then(|n| n.checked_mul(4));
        match (expected, u64::try_from(premultiplied.len())) {
            (Some(expected), Ok(len)) if expected == len => {}
            _ => return Err(format!("{width} × {height} pixels in {} bytes", premultiplied.len())),
        }
        let path = self.dir.join(format!("{}-{}", self.run, self.next));
        self.next = self.next.wrapping_add(1);
        // New, or an error: never an existing file, nor one a symlink points to.
        let file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Err(e) = write_straight(file, premultiplied) {
            let _ = std::fs::remove_file(&path);
            return Err(format!("{}: {e}", path.display()));
        }
        Ok(path)
    }
}

impl Drop for TempFiles {
    fn drop(&mut self) {
        if let Probe::Waiting { path, .. } = &self.probe {
            let _ = std::fs::remove_file(path);
        }
        for w in &self.waiting {
            let _ = std::fs::remove_file(&w.path);
        }
    }
}

/// The temporary directory: `TMPDIR` when set, else `/dev/shm` where there is one (memory, never
/// disk), else `/tmp`. Only an absolute path: the terminal would read a relative one from its own
/// working directory.
fn temp_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TMPDIR").filter(|d| !d.is_empty()) {
        let dir = PathBuf::from(dir);
        return dir.is_absolute().then_some(dir);
    }
    let shm = Path::new("/dev/shm");
    Some(if shm.is_dir() { shm.into() } else { PathBuf::from("/tmp") })
}

/// This user's directory in `parent`, made when missing, or `None` when it isn't safe: `parent`
/// must be a directory of this user's or root's that others can't write to, or can only add
/// their own entries to (sticky, as `/tmp`), so no one else can move the directory away; and the
/// directory must be this user's, not a symlink, and closed to everyone else.
fn own_dir(parent: &Path, uid: u32) -> Option<PathBuf> {
    let meta = std::fs::metadata(parent).ok()?;
    let others_write = meta.mode() & 0o022 != 0;
    let sticky = meta.mode() & 0o1000 != 0;
    if !meta.is_dir() || (meta.uid() != uid && meta.uid() != 0) || (others_write && !sticky) {
        return None;
    }
    let dir = parent.join(format!("{DIR_PREFIX}{uid}"));
    match DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    (meta.is_dir() && meta.uid() == uid && meta.mode() & 0o077 == 0).then_some(dir)
}

/// Whether `path` is still there (not followed if it is a symlink). A file that can't be looked
/// at counts as there: it is removed by name in the end either way.
fn present(path: &Path) -> bool {
    !matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == ErrorKind::NotFound)
}

/// `premultiplied` into `file` with straight alpha, as kitty's `f=32` has it.
fn write_straight(mut file: File, premultiplied: &[u8]) -> std::io::Result<()> {
    let mut chunk = Vec::with_capacity(WRITE_CHUNK.min(premultiplied.len()));
    for part in premultiplied.chunks(WRITE_CHUNK) {
        chunk.clear();
        chunk.extend_from_slice(part);
        unpremultiply(&mut chunk);
        file.write_all(&chunk)?;
    }
    Ok(())
}

/// Remove the files that runs killed before they could remove them left in `dir`: named
/// `<run>-<n>`, regular files of this user's, older than [`LEFT_BEHIND`], and their run's process
/// gone. A run that is only stopped keeps its files.
fn sweep(dir: &Path, uid: u32, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.take(MAX_SWEPT).flatten() {
        let Some(run) = entry.file_name().to_str().and_then(run_of) else { continue };
        // Of the entry itself, never what a symlink points to.
        let Ok(meta) = entry.metadata() else { continue };
        let old = meta.modified().ok().and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age >= LEFT_BEHIND);
        if meta.is_file() && meta.uid() == uid && old && gone(run) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The run (process id) a file name `<run>-<n>` belongs to: both parts digits, the run positive.
fn run_of(name: &str) -> Option<i32> {
    let (run, n) = name.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(run) || !digits(n) {
        return None;
    }
    run.parse::<i32>().ok().filter(|run| *run > 0)
}

/// Whether process `pid` (positive) no longer exists.
fn gone(pid: i32) -> bool {
    rustix::process::Pid::from_raw(pid).is_some_and(|pid| rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::PermissionsExt as _;

    fn uid() -> u32 {
        rustix::process::getuid().as_raw()
    }

    /// A temporary directory of the test's own, empty.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-cli-view-files-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    /// The files in this user's directory in `parent`.
    fn listing(parent: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(parent.join(format!("{DIR_PREFIX}{}", uid())))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The path a file transmission names.
    fn named(codes: &[u8]) -> PathBuf {
        let text = std::str::from_utf8(codes).unwrap();
        let payload = text.split(';').nth(1).unwrap().split('\x1b').next().unwrap();
        PathBuf::from(OsStr::from_bytes(&base64::engine::general_purpose::STANDARD.decode(payload).unwrap()))
    }

    /// A rendered page of `width` × `height` opaque grey pixels.
    fn page(width: u32, height: u32) -> RenderedPage {
        let request =
            pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false };
        let rgba = pdfcraft_render::Pixels::from(vec![0xFF80_8080; (width * height) as usize]);
        RenderedPage { request, width, height, rgba, error: None, text: None, warnings: Vec::new(), millis: 0 }
    }

    /// Started in `parent` as run 7, the probe already taken.
    fn taken(parent: &Path, now: Instant) -> TempFiles {
        let (files, codes) = TempFiles::start_in(parent, uid(), 7, 99, Wrap::None, now).unwrap();
        std::fs::remove_file(named(&codes)).unwrap();
        files
    }

    #[test]
    fn a_terminal_that_takes_the_probe_gets_pages_as_files() {
        let parent = scratch("taken");
        let now = Instant::now();
        let (mut files, codes) = TempFiles::start_in(&parent, uid(), 7, 99, Wrap::None, now).unwrap();
        // The probe: one pixel through a file, its image deleted straight after.
        let text = String::from_utf8_lossy(&codes);
        assert!(text.starts_with("\x1b_Ga=t,f=32,t=t,s=1,v=1,q=2,i=99;"), "{text:?}");
        assert!(text.ends_with("\x1b_Ga=d,d=I,q=2,i=99\x1b\\"), "{text:?}");
        let probe = named(&codes);
        let dir = parent.join(format!("tty-graphics-protocol-pdfcraft-{}", uid()));
        assert_eq!(probe, dir.join("7-0"));
        assert_eq!(std::fs::read(&probe).unwrap(), [255; 4]);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700, "this user's alone");
        assert_eq!(std::fs::metadata(&probe).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(!files.ready(now), "not before the terminal takes the probe");
        // The terminal reads it and deletes it.
        std::fs::remove_file(&probe).unwrap();
        assert!(files.ready(now));

        // A page: straight RGBA, half-covered red made whole again.
        let page = [64, 0, 0, 128, 10, 20, 30, 255];
        let path = files.write(5, 2, 1, &page, now).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), [128, 0, 0, 128, 10, 20, 30, 255]);
        assert!(files.write(6, 3, 1, &page, now).is_err(), "pixels that don't fill the size");
        assert!(!files.ready(now), "after a failure, the stream");
        drop(files);
        assert!(listing(&parent).is_empty(), "the files the terminal never took are removed: {:?}", listing(&parent));
    }

    #[test]
    fn a_terminal_that_leaves_the_probe_gets_the_stream() {
        let parent = scratch("refused");
        let now = Instant::now();
        let (mut files, _) = TempFiles::start_in(&parent, uid(), 7, 99, Wrap::Tmux, now).unwrap();
        assert!(!files.ready(now + PROBE_WAIT - Duration::from_millis(1)));
        assert_eq!(listing(&parent).len(), 1, "still waiting");
        // The wait is over: the stream, for good, and the probe is removed.
        assert!(!files.ready(now + PROBE_WAIT));
        assert!(listing(&parent).is_empty());
        assert!(!files.ready(now + PROBE_WAIT * 2));
    }

    #[test]
    fn files_the_terminal_leaves_are_removed_and_their_images_named() {
        let parent = scratch("stale");
        let now = Instant::now();
        let mut files = taken(&parent, now);
        let took = files.write(1, 1, 1, &[0; 4], now).unwrap();
        let left = files.write(2, 1, 1, &[0; 4], now).unwrap();
        std::fs::remove_file(&took).unwrap();
        assert_eq!(files.check(now + STALE - Duration::from_millis(1)), Vec::<u32>::new());
        assert_eq!(files.waiting.len(), 1, "the taken one is forgotten");
        assert!(files.ready(now + STALE - Duration::from_millis(1)));
        assert_eq!(files.check(now + STALE), [2]);
        assert!(!left.exists());
        assert!(files.waiting.is_empty());
        // The terminal doesn't take files after all: the stream from now on.
        assert!(!files.ready(now + STALE));
    }

    #[test]
    fn too_many_files_waiting_send_pages_in_the_stream_until_it_catches_up() {
        let parent = scratch("behind");
        let now = Instant::now();
        let mut files = taken(&parent, now);
        let paths: Vec<PathBuf> = (0..MAX_WAITING as u32).map(|id| files.write(id, 1, 1, &[0; 4], now).unwrap()).collect();
        assert!(!files.ready(now));
        std::fs::remove_file(&paths[0]).unwrap();
        files.check(now);
        assert!(files.ready(now));
    }

    #[test]
    fn files_tmux_threw_away_are_removed_without_giving_files_up() {
        let parent = scratch("forget");
        let now = Instant::now();
        let mut files = taken(&parent, now);
        let path = files.write(1, 1, 1, &[0; 4], now).unwrap();
        files.forget();
        assert!(!path.exists());
        assert_eq!(files.check(now + STALE), Vec::<u32>::new());
        assert!(files.ready(now + STALE), "pages still go through files");
    }

    #[test]
    fn the_files_work_on_a_thread_of_their_own() {
        let parent = scratch("thread");
        let start = {
            let parent = parent.clone();
            move || TempFiles::start_in(&parent, uid(), 7, 99, Wrap::None, Instant::now())
        };
        let (mut files, codes) = Files::spawn(STALL, start).unwrap();
        let now = Instant::now();
        assert!(!files.ready(now));
        std::fs::remove_file(named(&codes)).unwrap();
        assert!(files.ready(now));
        let Written::File(path) = files.write(5, page(2, 1), now) else { panic!("no file") };
        assert_eq!(std::fs::read(&path).unwrap(), [0x80, 0x80, 0x80, 0xFF].repeat(2));
        assert_eq!(files.check(now), Vec::<u32>::new());
        assert_eq!(files.waiting, [5]);
        // A page that can't be written comes back, to go in the stream.
        let mut odd = page(2, 1);
        odd.width = 3;
        assert!(matches!(files.write(6, odd, now), Written::Failed(_, back) if back.width == 3));
        assert!(!files.ready(now));
        // Dropped: the thread removes what is left before this returns.
        drop(files);
        assert!(listing(&parent).is_empty(), "{:?}", listing(&parent));
        // Files that can't be: none, and no wait.
        assert!(Files::spawn(STALL, || None).is_none());
    }

    #[test]
    fn a_thread_that_was_only_slow_removes_its_files_before_the_pager_ends() {
        let parent = scratch("slow");
        let start = {
            let parent = parent.clone();
            move || TempFiles::start_in(&parent, uid(), 7, 99, Wrap::None, Instant::now())
        };
        let (mut files, codes) = Files::spawn(STALL, start).unwrap();
        std::fs::remove_file(named(&codes)).unwrap();
        let now = Instant::now();
        assert!(files.ready(now));
        // A write answered too late (a loaded machine): files are given up, while the thread
        // goes on and writes the file.
        files.stall = Duration::ZERO;
        assert!(matches!(files.write(5, page(2048, 2048), now), Written::Stalled));
        assert!(files.stalled());
        let deadline = Instant::now() + Duration::from_secs(5);
        while listing(&parent).is_empty() {
            assert!(Instant::now() < deadline, "the thread never started the file");
            std::thread::sleep(Duration::from_millis(1));
        }
        // The pager ends while the file is still being written: the thread still removes it
        // before this returns.
        files.stall = STALL;
        drop(files);
        assert!(listing(&parent).is_empty(), "{:?}", listing(&parent));
    }

    #[test]
    fn a_stalled_thread_gives_files_up_and_never_stalls_the_pager() {
        // A start that hangs: one wait, not two.
        let slow = Duration::from_millis(300);
        let started = Instant::now();
        let hangs = || {
            std::thread::sleep(Duration::from_secs(2));
            None
        };
        assert!(Files::spawn(slow, hangs).is_none());
        assert!(started.elapsed() < slow + slow / 2, "{:?}", started.elapsed());
        let stall = Duration::from_millis(50);
        // The pager's end of a thread this test plays: it answers a look (one file waiting), then
        // stalls in the middle of writing a page.
        let (jobs, inbox) = mpsc::channel();
        let (outbox, answers) = mpsc::channel();
        let mut files = Files { jobs, answers, stall, waiting: Vec::new(), stalled: false };
        outbox.send(Answer::Checked { lost: Vec::new(), waiting: vec![4] }).unwrap();
        assert_eq!(files.check(Instant::now()), Vec::<u32>::new());
        assert!(matches!(files.write(5, page(1, 1), Instant::now()), Written::Stalled));
        assert!(files.stalled());
        // From then on nothing waits: files are over, and the pages in files the terminal hadn't
        // taken are lost, once.
        let after = Instant::now();
        assert!(!files.ready(Instant::now()));
        assert_eq!(files.check(Instant::now()), [4, 5]);
        assert_eq!(files.check(Instant::now()), Vec::<u32>::new());
        assert!(after.elapsed() < stall);
        // At the end, the thread is told to stop and waited for, at most once more.
        drop(files);
        assert!(after.elapsed() < stall * 3);
        let asked = inbox.try_iter().collect::<Vec<_>>();
        assert!(matches!(asked.as_slice(), [Job::Check(_), Job::Write { .. }, Job::Stop]), "nothing else asked after the stall");
        drop(outbox);
    }

    #[test]
    fn a_planted_file_or_symlink_is_never_written_through() {
        let parent = scratch("planted");
        let target = parent.join("target");
        std::fs::write(&target, b"keep").unwrap();
        // Someone guessed the probe's name and put a symlink there.
        let dir = own_dir(&parent, uid()).unwrap();
        std::os::unix::fs::symlink(&target, dir.join("7-0")).unwrap();
        assert!(TempFiles::start_in(&parent, uid(), 7, 99, Wrap::None, Instant::now()).is_none());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }

    #[test]
    fn the_directory_must_be_safe_from_other_users() {
        // This user's directory as a symlink to somewhere else: refused.
        let parent = scratch("unsafe");
        let elsewhere = parent.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, parent.join(format!("{DIR_PREFIX}{}", uid()))).unwrap();
        assert_eq!(own_dir(&parent, uid()), None);
        // Open to others: refused, until closed again.
        let parent = scratch("open");
        let dir = own_dir(&parent, uid()).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(own_dir(&parent, uid()), None);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(own_dir(&parent, uid()), Some(dir));
        // A temporary directory anyone can write to and move things out of: refused; sticky, as
        // /tmp is: fine.
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(own_dir(&parent, uid()), None);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o1777)).unwrap();
        assert!(own_dir(&parent, uid()).is_some());
        // Another user's directory (as seen by someone else).
        assert_eq!(own_dir(&parent, uid().wrapping_add(1)), None);
    }

    #[test]
    fn only_files_of_runs_that_are_gone_are_swept() {
        let parent = scratch("sweep");
        let dir = own_dir(&parent, uid()).unwrap();
        // A process that has exited.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        let dead = child.id();
        let alive = std::process::id();
        for name in [format!("{dead}-0"), format!("{dead}-1"), format!("{alive}-0"), format!("{dead}-x"), "notes.txt".into(), "0-0".into()] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.join(format!("{dead}-2"))).unwrap();
        // Just written: kept, even a gone run's.
        sweep(&dir, uid(), SystemTime::now());
        assert_eq!(listing(&parent).len(), 7);
        // A minute on: the gone run's files go; a live run's, other names and directories stay.
        sweep(&dir, uid(), SystemTime::now() + LEFT_BEHIND + Duration::from_secs(1));
        let mut kept = vec![format!("{alive}-0"), format!("{dead}-x"), "notes.txt".into(), "0-0".into(), format!("{dead}-2")];
        kept.sort();
        assert_eq!(listing(&parent), kept);
    }

    #[test]
    fn file_names_name_their_run() {
        assert_eq!(run_of("123-0"), Some(123));
        assert_eq!(run_of("123-45"), Some(123));
        for bad in ["0-1", "-1-2", "123", "123-", "-4", "12a-1", "1-2-3", "99999999999-1", "+5-1", " 5-1"] {
            assert_eq!(run_of(bad), None, "{bad}");
        }
    }

    #[test]
    fn large_pages_are_written_whole() {
        let parent = scratch("large");
        let now = Instant::now();
        let mut files = taken(&parent, now);
        // More than one chunk, and not a whole number of them.
        let (w, h) = (300u32, 301u32);
        let page: Vec<u8> = (0..w * h * 4).map(|i| if i % 4 == 3 { 255 } else { (i % 251) as u8 }).collect();
        let path = files.write(1, w, h, &page, now).unwrap();
        assert!(page.len() > WRITE_CHUNK && !page.len().is_multiple_of(WRITE_CHUNK));
        assert_eq!(std::fs::read(&path).unwrap(), page, "opaque pixels as they are");
    }
}
