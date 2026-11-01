//! Search the text of every PDF in a folder, one file at a time (Edit ▸ Search a folder…). Nothing
//! is written. A file that can't be searched is named with its reason, never passed over.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use pdfcraft_render::{OpenError, PageText, RenderConfig, RenderPool, RenderRequest, RequestKind, Retiring, inspect};

/// Files and folders looked at, at most, as in a batch job.
pub const MAX_ENTRIES: usize = 10_000;
/// Folder levels followed down, at most, as in a batch job.
pub const MAX_DEPTH: usize = 64;
/// The largest file read, as for a tool's input.
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Matches kept unless the caller asks otherwise.
pub const DEFAULT_LIMIT: usize = 1_000;
/// The longest query taken.
pub const MAX_QUERY_CHARS: usize = 1_000;
/// Files that may run out of time before the search stops: each can leave a stuck thread behind.
pub const MAX_SLOW_FILES: usize = 3;
/// Threads earlier searches left running, at most, before a new search is refused.
pub const MAX_LEFT_RUNNING: usize = 3 * MAX_SLOW_FILES;
/// How long a file may take to open, and one page to read.
pub const TIME_LIMIT: Duration = Duration::from_secs(20);
/// Pages asked for at once.
const PAGE_BATCH: usize = 16;
/// Glyphs of context on each side of a match.
const CONTEXT: usize = 36;

/// Threads a search let go of. They can't be killed, so each counts until it exits.
static LEFT_RUNNING: Mutex<Vec<Retiring>> = Mutex::new(Vec::new());

/// What to look for, and where.
#[derive(Clone, Debug)]
pub struct Options {
    pub folder: PathBuf,
    pub query: String,
    pub subfolders: bool,
    pub case_sensitive: bool,
    pub whole_words: bool,
    /// Stop after this many matches.
    pub limit: usize,
    /// How long a file may take to open.
    pub open_limit: Duration,
    /// How long one page may take to read.
    pub page_limit: Duration,
}

impl Options {
    pub fn new(folder: impl Into<PathBuf>, query: impl Into<String>) -> Self {
        Self {
            folder: folder.into(),
            query: query.into(),
            subfolders: true,
            case_sensitive: false,
            whole_words: false,
            limit: DEFAULT_LIMIT,
            open_limit: TIME_LIMIT,
            page_limit: TIME_LIMIT,
        }
    }
}

/// One match, with the text around it on its page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// 0-based.
    pub page: usize,
    pub before: String,
    pub text: String,
    pub after: String,
}

/// The matches in one file, in page order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatches {
    pub path: PathBuf,
    pub matches: Vec<Match>,
}

/// Why a file was not searched, or not all of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// Links are not followed, so a search stays inside its folder and can't go round in circles.
    LinkOrSpecialFile,
    TooLarge,
    Unreadable(String),
    Damaged(String),
    /// Encrypted, even with no password to open it: its text may not be for copying.
    Secured,
    /// Ran out of time; any matches before that are kept.
    TooSlow,
    /// These many pages could not be read.
    PagesUnread(usize),
}

impl Skip {
    /// A stable name for tools.
    pub fn name(&self) -> &'static str {
        match self {
            Self::LinkOrSpecialFile => "link_or_special_file",
            Self::TooLarge => "too_large",
            Self::Unreadable(_) => "unreadable",
            Self::Damaged(_) => "damaged",
            Self::Secured => "secured",
            Self::TooSlow => "too_slow",
            Self::PagesUnread(_) => "pages_unread",
        }
    }
}

/// A file, or a folder that could not be listed, and why it was left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    pub reason: Skip,
}

/// Why a search ended before the last file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Cancelled,
    MatchLimit,
    TooManySlowFiles,
}

/// What a search has found so far, as it happens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The folder has been listed: this many PDFs to search.
    Found(usize),
    /// File `index` of those is being read.
    Reading {
        index: usize,
        path: PathBuf,
    },
    Matches(FileMatches),
    Skipped(Skipped),
}

/// How a search ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// PDFs found in the folder.
    pub found: usize,
    /// Those read to the end.
    pub searched: usize,
    pub files_with_matches: usize,
    pub matches: usize,
    /// The folder held more than [`MAX_ENTRIES`] or went deeper than [`MAX_DEPTH`].
    pub folder_cut_short: bool,
    pub stopped: Option<Stop>,
}

/// Whether `metadata`, read without following links, is a link (or a junction on Windows).
pub fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Search every PDF in `options.folder`. `events` hears each result as it is found, and `cancel`
/// ends the search at the next page. An `Err` means nothing was searched.
pub fn search(options: &Options, cancel: &AtomicBool, mut events: impl FnMut(Event)) -> Result<Summary, String> {
    crate::guard(|| search_inner(options, cancel, &mut events, &LEFT_RUNNING))?
}

fn search_inner(options: &Options, cancel: &AtomicBool, events: &mut impl FnMut(Event), left: &Mutex<Vec<Retiring>>) -> Result<Summary, String> {
    let query = options.query.trim();
    if query.is_empty() {
        return Err("the search text is empty".into());
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(format!("the search text is longer than {MAX_QUERY_CHARS} characters"));
    }
    let folder = options.folder.symlink_metadata().map_err(|e| format!("{}: {e}", options.folder.display()))?;
    if !folder.is_dir() {
        return Err(format!("{} is not a folder", options.folder.display()));
    }
    if running(left) >= MAX_LEFT_RUNNING {
        return Err("files from earlier searches are still being read; wait for them to finish, or restart PdfCraft".into());
    }
    let mut walk = Walk::default();
    walk.folder(&options.folder, options.subfolders, 0);
    let mut summary = Summary { found: walk.pdfs.len(), folder_cut_short: walk.cut_short, ..Summary::default() };
    events(Event::Found(summary.found));
    for skipped in walk.skipped {
        events(Event::Skipped(skipped));
    }
    let mut slow = 0usize;
    for (index, path) in walk.pdfs.into_iter().enumerate() {
        if summary.matches >= options.limit {
            summary.stopped = Some(Stop::MatchLimit);
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            summary.stopped = Some(Stop::Cancelled);
            break;
        }
        events(Event::Reading { index, path: path.clone() });
        let read = search_file(&path, query, options, options.limit.saturating_sub(summary.matches), cancel, left);
        if !read.matches.is_empty() {
            summary.matches = summary.matches.saturating_add(read.matches.len());
            summary.files_with_matches = summary.files_with_matches.saturating_add(1);
            events(Event::Matches(FileMatches { path: path.clone(), matches: read.matches }));
        }
        if read.cancelled {
            summary.stopped = Some(Stop::Cancelled);
            break;
        }
        match read.skip {
            Some(reason) => {
                slow = slow.saturating_add(usize::from(reason == Skip::TooSlow));
                events(Event::Skipped(Skipped { path, reason }));
            }
            None if read.at_limit => {}
            None => summary.searched = summary.searched.saturating_add(1),
        }
        if read.at_limit {
            summary.stopped = Some(Stop::MatchLimit);
            break;
        }
        if slow >= MAX_SLOW_FILES {
            summary.stopped = Some(Stop::TooManySlowFiles);
            break;
        }
    }
    Ok(summary)
}

#[derive(Default)]
struct Walk {
    pdfs: Vec<PathBuf>,
    skipped: Vec<Skipped>,
    seen: usize,
    cut_short: bool,
}

impl Walk {
    fn folder(&mut self, folder: &Path, subfolders: bool, depth: usize) {
        let entries = match std::fs::read_dir(folder) {
            Ok(entries) => entries,
            Err(error) => return self.skip(folder, Skip::Unreadable(error.to_string())),
        };
        // Bound the list before sorting it; never collect an unbounded folder listing.
        let mut paths = Vec::new();
        for entry in entries {
            if self.seen >= MAX_ENTRIES {
                self.cut_short = true;
                break;
            }
            self.seen += 1;
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(error) => self.skip(folder, Skip::Unreadable(error.to_string())),
            }
        }
        paths.sort();
        for path in paths {
            let metadata = match path.symlink_metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.skip(&path, Skip::Unreadable(error.to_string()));
                    continue;
                }
            };
            if linked(&metadata) || (!metadata.is_file() && !metadata.is_dir()) {
                self.skip(&path, Skip::LinkOrSpecialFile);
            } else if metadata.is_file() {
                if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("pdf")) {
                    self.pdfs.push(path);
                }
            } else if subfolders && depth >= MAX_DEPTH {
                self.cut_short = true;
            } else if subfolders {
                self.folder(&path, subfolders, depth + 1);
            }
        }
    }

    fn skip(&mut self, path: &Path, reason: Skip) {
        self.skipped.push(Skipped { path: path.to_path_buf(), reason });
    }
}

#[derive(Default)]
struct FileRead {
    matches: Vec<Match>,
    skip: Option<Skip>,
    cancelled: bool,
    /// The match limit ended the file with some of it unread.
    at_limit: bool,
}

fn search_file(path: &Path, query: &str, options: &Options, room: usize, cancel: &AtomicBool, left: &Mutex<Vec<Retiring>>) -> FileRead {
    let mut read = FileRead::default();
    let bytes = match read_bytes(path) {
        Ok(bytes) => Arc::new(bytes),
        Err(skip) => {
            read.skip = Some(skip);
            return read;
        }
    };
    let pages = match open(&bytes, options.open_limit, cancel, left) {
        Opened::Pages(pages) => pages,
        Opened::Skip(skip) => {
            read.skip = Some(skip);
            return read;
        }
        Opened::Cancelled => {
            read.cancelled = true;
            return read;
        }
    };
    read_pages(bytes, pages, query, options, room, cancel, left)
}

/// How many of the threads let go of are still running.
fn running(left: &Mutex<Vec<Retiring>>) -> usize {
    let mut left = left.lock().unwrap_or_else(PoisonError::into_inner);
    left.retain_mut(|threads| threads.running() > 0);
    left.iter_mut().map(Retiring::running).sum()
}

fn let_go(left: &Mutex<Vec<Retiring>>, threads: Retiring) {
    left.lock().unwrap_or_else(PoisonError::into_inner).push(threads);
}

fn read_pages(
    bytes: Arc<Vec<u8>>,
    pages: usize,
    query: &str,
    options: &Options,
    room: usize,
    cancel: &AtomicBool,
    left: &Mutex<Vec<Retiring>>,
) -> FileRead {
    // The pool, not a bare renderer: its watchdog stops a page that would otherwise never end.
    let mut pool = RenderPool::new(bytes, 1, RenderConfig::default());
    pool.set_stuck_after(options.page_limit);
    let read = read_queued(&mut pool, pages, query, options, room, cancel);
    // With every page answered its worker is idle; otherwise it may be on a page that never ends.
    if read.cancelled || read.at_limit || read.skip == Some(Skip::TooSlow) {
        let_go(left, pool.retire());
    }
    read
}

fn read_queued(pool: &mut RenderPool, pages: usize, query: &str, options: &Options, room: usize, cancel: &AtomicBool) -> FileRead {
    let mut read = FileRead::default();
    let mut unread = 0usize;
    let mut first = 0;
    'pages: while first < pages {
        let end = first.saturating_add(PAGE_BATCH).min(pages);
        pool.set_queue(
            (first..end).map(|page| RenderRequest { page, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, preview: false }).collect(),
        );
        for _ in first..end {
            if read.matches.len() >= room {
                read.at_limit = true;
                break 'pages;
            }
            let page = loop {
                if cancel.load(Ordering::Relaxed) {
                    read.cancelled = true;
                    return read;
                }
                if let Some(page) = pool.try_recv() {
                    break page;
                }
                std::thread::sleep(Duration::from_millis(1));
            };
            match (page.error, page.text) {
                (Some(_), _) if pool.timed_out() => {
                    read.skip = Some(Skip::TooSlow);
                    return read;
                }
                (Some(_), _) => unread = unread.saturating_add(1),
                (None, Some(text)) => {
                    if collect(&text, page.request.page, query, options, room, &mut read.matches) {
                        read.at_limit = true;
                        break 'pages;
                    }
                }
                (None, None) => {}
            }
        }
        first = end;
    }
    read.skip = (unread > 0).then_some(Skip::PagesUnread(unread));
    read
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, Skip> {
    let unreadable = |error: std::io::Error| Skip::Unreadable(error.to_string());
    let file = std::fs::File::open(path).map_err(unreadable)?;
    if file.metadata().map_err(unreadable)?.len() > MAX_FILE_BYTES {
        return Err(Skip::TooLarge);
    }
    let mut bytes = Vec::new();
    // It may have grown since: never read past the limit.
    file.take(MAX_FILE_BYTES.saturating_add(1)).read_to_end(&mut bytes).map_err(unreadable)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(Skip::TooLarge);
    }
    Ok(bytes)
}

enum Opened {
    Pages(usize),
    Skip(Skip),
    Cancelled,
}

/// Open the file on a thread of its own: the render pool's watchdog doesn't cover parsing.
fn open(bytes: &Arc<Vec<u8>>, limit: Duration, cancel: &AtomicBool, left: &Mutex<Vec<Retiring>>) -> Opened {
    let (sender, receiver) = mpsc::channel();
    let bytes = bytes.clone();
    let spawned = std::thread::Builder::new().name("folder-search-open".into()).spawn(move || {
        let _ = sender.send(inspect(bytes, None));
    });
    let thread = match spawned {
        Ok(thread) => thread,
        Err(error) => return Opened::Skip(Skip::Unreadable(error.to_string())),
    };
    let started = Instant::now();
    let gave_up = loop {
        if cancel.load(Ordering::Relaxed) {
            break Opened::Cancelled;
        }
        if started.elapsed() >= limit {
            break Opened::Skip(Skip::TooSlow);
        }
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(Ok(info)) if info.encrypted => return Opened::Skip(Skip::Secured),
            Ok(Ok(info)) => return Opened::Pages(info.pages.len()),
            Ok(Err(OpenError::NeedsPassword | OpenError::WrongPassword)) => return Opened::Skip(Skip::Secured),
            Ok(Err(error)) => return Opened::Skip(Skip::Damaged(error.to_string())),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Opened::Skip(Skip::Damaged("the reader stopped unexpectedly".into())),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    };
    let_go(left, thread.into());
    gave_up
}

/// Add the page's matches, up to `room` in all. Returns whether any had to be left out.
fn collect(text: &PageText, page: usize, query: &str, options: &Options, room: usize, matches: &mut Vec<Match>) -> bool {
    let glyphs = text.glyphs.len();
    let left = room.saturating_sub(matches.len());
    // One more than there is room for proves that some were left out.
    let found = text.find_opts_limited(query, options.case_sensitive, options.whole_words, left.saturating_add(1));
    let more = found.len() > left;
    for range in found.into_iter().take(left) {
        let (start, end) = (range.start.min(glyphs), range.end.min(glyphs));
        matches.push(Match {
            page,
            before: excerpt(text, start.saturating_sub(CONTEXT)..start),
            text: excerpt(text, start..end),
            after: excerpt(text, end..end.saturating_add(CONTEXT).min(glyphs)),
        });
    }
    more
}

fn excerpt(text: &PageText, range: std::ops::Range<usize>) -> String {
    text.text_of(range).split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
