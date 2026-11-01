use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use pdfcraft_render::Retiring;
use pdfcraft_testkit::Pdf;

use super::{
    Event, FileMatches, MAX_DEPTH, MAX_ENTRIES, MAX_FILE_BYTES, MAX_LEFT_RUNNING, MAX_QUERY_CHARS, Options, Skip, Skipped, Stop, Summary, search,
};

fn raw_pdf(contents: &[String]) -> Vec<u8> {
    let kids: Vec<String> = (0..contents.len()).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    let mut pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj(format!("<< /Type /Pages /Kids [{}] /Count {} /MediaBox [0 0 400 300] >>", kids.join(" "), contents.len()))
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    for (i, content) in contents.iter().enumerate() {
        pdf = pdf
            .obj(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i))
            .stream("", content);
    }
    pdf.build()
}

fn pdf(pages: &[&str]) -> Vec<u8> {
    raw_pdf(&pages.iter().map(|text| format!("BT /F1 12 Tf 20 150 Td ({text}) Tj ET")).collect::<Vec<_>>())
}

/// Long enough for a first page on a busy machine, and far short of `slow_pdf`'s second.
const PAGE_LIMIT: Duration = Duration::from_millis(300);

/// A second page that takes about 1.4 s to read, far longer than `PAGE_LIMIT`.
fn slow_pdf(before: &str) -> Vec<u8> {
    let mut content = format!("BT /F1 12 Tf 20 150 Td ({before}) Tj\n");
    content.push_str(&"(x) Tj\n".repeat(4_000_000));
    content.push_str("ET");
    raw_pdf(&[format!("BT /F1 12 Tf 20 150 Td ({before}) Tj ET"), content])
}

struct Folder(PathBuf);

impl Folder {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pdfcraft-folder-search-{}-{n}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
struct Run {
    summary: Summary,
    files: Vec<FileMatches>,
    skipped: Vec<Skipped>,
    reading: Vec<PathBuf>,
}

fn run(options: &Options) -> Run {
    let cancel = AtomicBool::new(false);
    let mut out = Run { summary: Summary::default(), files: Vec::new(), skipped: Vec::new(), reading: Vec::new() };
    out.summary = search(options, &cancel, |event| match event {
        Event::Found(_) => {}
        Event::Reading { path, .. } => out.reading.push(path),
        Event::Matches(file) => out.files.push(file),
        Event::Skipped(skipped) => out.skipped.push(skipped),
    })
    .unwrap();
    out
}

fn reasons<'a>(run: &'a Run, path: &Path) -> Vec<&'a Skip> {
    run.skipped.iter().filter(|s| s.path == path).map(|s| &s.reason).collect()
}

fn none_left() -> Mutex<Vec<Retiring>> {
    Mutex::new(Vec::new())
}

fn kept(left: &Mutex<Vec<Retiring>>) -> usize {
    left.lock().unwrap().len()
}

/// A thread that runs until the returned sender is dropped.
fn hold_a_thread(left: &Mutex<Vec<Retiring>>) -> mpsc::Sender<()> {
    let (release, wait) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        let _ = wait.recv();
    });
    super::let_go(left, thread.into());
    release
}

#[test]
fn matches_come_by_file_and_page_with_the_text_around_them() {
    let folder = Folder::new();
    let a = folder.write("a.pdf", &pdf(&["nothing here", "the quick brown fox"]));
    let b = folder.write("sub/b.pdf", &pdf(&["a quick look"]));
    folder.write("c.pdf", &pdf(&["no match"]));
    let found = run(&Options::new(&folder.0, "quick"));
    assert_eq!(found.summary.found, 3);
    assert_eq!(found.summary.searched, 3);
    assert_eq!(found.summary.files_with_matches, 2);
    assert_eq!(found.summary.matches, 2);
    assert_eq!(found.summary.stopped, None);
    assert_eq!(found.files[0].path, a);
    assert_eq!(found.files[0].matches[0].page, 1, "the second page, counted from 0");
    assert_eq!(found.files[0].matches[0].before, "the");
    assert_eq!(found.files[0].matches[0].text, "quick");
    assert_eq!(found.files[0].matches[0].after, "brown fox");
    assert_eq!(found.files[1].path, b);
}

#[test]
fn a_query_is_trimmed_and_its_options_apply() {
    let folder = Folder::new();
    folder.write("a.pdf", &pdf(&["Cat Cat category"]));
    assert_eq!(run(&Options::new(&folder.0, "  cat ")).summary.matches, 3, "any case, any part of a word");
    let mut case = Options::new(&folder.0, "cat");
    case.case_sensitive = true;
    assert_eq!(run(&case).summary.matches, 1);
    let mut words = Options::new(&folder.0, "cat");
    words.whole_words = true;
    assert_eq!(run(&words).summary.matches, 2);
}

#[test]
fn subfolders_can_be_left_out() {
    let folder = Folder::new();
    folder.write("a.pdf", &pdf(&["needle"]));
    folder.write("sub/b.pdf", &pdf(&["needle"]));
    let mut options = Options::new(&folder.0, "needle");
    assert_eq!(run(&options).summary.found, 2);
    options.subfolders = false;
    assert_eq!(run(&options).summary.found, 1);
}

#[test]
fn only_pdfs_are_read_whatever_the_case_of_their_extension() {
    let folder = Folder::new();
    let upper = folder.write("A.PDF", &pdf(&["needle"]));
    folder.write("notes.txt", b"needle");
    folder.write("pdf", &pdf(&["needle"]));
    let found = run(&Options::new(&folder.0, "needle"));
    assert_eq!(found.reading, [upper]);
    assert!(found.skipped.is_empty(), "other files are not worth a mention: {:?}", found.skipped);
}

#[cfg(unix)]
#[test]
fn links_are_named_and_never_followed() {
    let outside = Folder::new();
    let secret = outside.write("secret.pdf", &pdf(&["needle"]));
    let folder = Folder::new();
    folder.write("a.pdf", &pdf(&["needle"]));
    let to_file = folder.0.join("link.pdf");
    std::os::unix::fs::symlink(&secret, &to_file).unwrap();
    let to_folder = folder.0.join("outside");
    std::os::unix::fs::symlink(&outside.0, &to_folder).unwrap();
    let to_itself = folder.0.join("loop");
    std::os::unix::fs::symlink(&folder.0, &to_itself).unwrap();
    let found = run(&Options::new(&folder.0, "needle"));
    assert_eq!(found.summary.found, 1, "only the real file");
    assert_eq!(found.summary.matches, 1);
    for link in [&to_file, &to_folder, &to_itself] {
        assert_eq!(reasons(&found, link), [&Skip::LinkOrSpecialFile], "{}", link.display());
    }
}

#[cfg(unix)]
#[test]
fn a_folder_that_is_a_link_is_refused() {
    let folder = Folder::new();
    let link = folder.0.join("link");
    let real = folder.0.join("real");
    fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let error = search(&Options::new(&link, "x"), &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(error.contains("not a folder"), "{error}");
}

#[test]
fn a_damaged_file_is_named_and_the_rest_are_still_searched() {
    let folder = Folder::new();
    let bad = folder.write("a-bad.pdf", b"this is not a PDF");
    folder.write("b.pdf", &pdf(&["needle"]));
    let found = run(&Options::new(&folder.0, "needle"));
    assert!(matches!(reasons(&found, &bad)[..], [Skip::Damaged(_)]), "{:?}", found.skipped);
    assert_eq!(found.summary.matches, 1);
    assert_eq!(found.summary.searched, 1, "the damaged file does not count as searched");
    assert_eq!(found.summary.found, 2);
}

#[test]
fn encrypted_files_are_named_and_never_searched() {
    let folder = Folder::new();
    let needs_a_password = folder.write("a.pdf", &crate::tests::protected("user", "owner", -4));
    // Opens with no password, and copying its text is not allowed.
    let no_copying = folder.write("b.pdf", &crate::tests::protected("", "owner", -20));
    let found = run(&Options::new(&folder.0, "Page"));
    assert_eq!(reasons(&found, &needs_a_password), [&Skip::Secured]);
    assert_eq!(reasons(&found, &no_copying), [&Skip::Secured]);
    assert!(found.files.is_empty(), "no text of theirs comes back: {:?}", found.files);
    assert_eq!(found.summary.searched, 0);
}

#[test]
fn a_file_past_the_size_limit_is_named_and_not_read() {
    let folder = Folder::new();
    let big = folder.write("big.pdf", b"");
    fs::File::options().write(true).open(&big).unwrap().set_len(MAX_FILE_BYTES + 1).unwrap();
    let at_the_limit = folder.write("edge.pdf", b"");
    fs::File::options().write(true).open(&at_the_limit).unwrap().set_len(MAX_FILE_BYTES).unwrap();
    let found = run(&Options::new(&folder.0, "needle"));
    assert_eq!(reasons(&found, &big), [&Skip::TooLarge]);
    assert!(matches!(reasons(&found, &at_the_limit)[..], [Skip::Damaged(_)]), "read, since it is not past the limit: {:?}", found.skipped);
}

#[test]
fn the_search_stops_at_the_match_limit_on_the_exact_count() {
    let folder = Folder::new();
    folder.write("a.pdf", &pdf(&["needle needle", "needle"]));
    let late = folder.write("b.pdf", &pdf(&["needle"]));
    let mut options = Options::new(&folder.0, "needle");
    options.limit = 2;
    let found = run(&options);
    assert_eq!(found.summary.matches, 2);
    assert_eq!(found.files[0].matches.len(), 2);
    assert_eq!(found.summary.stopped, Some(Stop::MatchLimit));
    assert!(!found.reading.contains(&late), "and goes no further");
    assert_eq!(found.summary.searched, 0, "a page of a.pdf was left unread");
    options.limit = 3;
    let first_file = run(&options);
    assert_eq!(first_file.summary.searched, 1, "a.pdf was read to its end");
    assert_eq!(first_file.summary.stopped, Some(Stop::MatchLimit), "and b.pdf was not read");
    assert!(!first_file.reading.contains(&late));
    options.limit = 4;
    let all = run(&options);
    assert_eq!(all.summary.matches, 4);
    assert_eq!(all.summary.searched, 2);
    assert_eq!(all.summary.stopped, None, "exactly the limit, and nothing left out");
}

#[test]
fn the_limit_says_so_when_it_cuts_a_page_short() {
    let folder = Folder::new();
    folder.write("a.pdf", &pdf(&["needle needle"]));
    let mut options = Options::new(&folder.0, "needle");
    options.limit = 1;
    let found = run(&options);
    assert_eq!(found.summary.matches, 1);
    assert_eq!(found.summary.stopped, Some(Stop::MatchLimit), "the last page held one more");
    assert_eq!(found.summary.searched, 0);
}

#[test]
fn the_limit_ends_a_file_at_the_page_that_filled_it() {
    let folder = Folder::new();
    folder.write("a.pdf", &pdf(&["needle", "nothing else"]));
    let mut options = Options::new(&folder.0, "needle");
    options.limit = 1;
    let found = run(&options);
    assert_eq!(found.summary.matches, 1);
    assert_eq!(found.summary.stopped, Some(Stop::MatchLimit), "the second page was never read");
    assert_eq!(found.summary.searched, 0);
}

#[test]
fn a_file_the_limit_ended_says_so() {
    let options = Options::new("unused", "needle");
    let bytes = Arc::new(pdf(&["needle needle"]));
    let left = none_left();
    let read = super::read_pages(bytes, 1, "needle", &options, 1, &AtomicBool::new(false), &left);
    assert!(read.at_limit);
    assert_eq!(read.matches.len(), 1);
    assert_eq!(kept(&left), 1, "its worker may be on a page nobody will read");
}

#[test]
fn cancelling_while_a_file_opens_or_its_pages_are_read_ends_at_once() {
    let bytes = Arc::new(pdf(&["needle"]));
    let cancelled = AtomicBool::new(true);
    let left = none_left();
    assert!(matches!(super::open(&bytes, Duration::from_secs(60), &cancelled, &left), super::Opened::Cancelled));
    assert_eq!(kept(&left), 1, "the thread that was opening it");
    let read = super::read_pages(bytes, 1, "needle", &Options::new("unused", "needle"), 10, &cancelled, &left);
    assert!(read.cancelled);
    assert!(read.matches.is_empty());
    assert_eq!(kept(&left), 2, "and the worker on its page");
}

#[test]
fn matches_in_later_pages_of_a_long_file_are_found() {
    let folder = Folder::new();
    let mut pages = vec!["filler"; 40];
    pages[39] = "needle";
    folder.write("a.pdf", &pdf(&pages));
    let found = run(&Options::new(&folder.0, "needle"));
    assert_eq!(found.files[0].matches[0].page, 39);
}

#[test]
fn cancelling_ends_the_search_at_the_next_step() {
    let folder = Folder::new();
    folder.write("a.pdf", &pdf(&["needle"]));
    folder.write("b.pdf", &pdf(&["needle"]));
    let cancel = AtomicBool::new(false);
    let mut read = 0;
    let summary = search(&Options::new(&folder.0, "needle"), &cancel, |event| {
        if matches!(event, Event::Reading { .. }) {
            read += 1;
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert_eq!(read, 1, "the second file is never started");
    assert_eq!(summary.stopped, Some(Stop::Cancelled));
    assert_eq!(summary.searched, 0, "the file it was reading isn't counted as read");
}

#[test]
fn cancelling_before_the_first_file_reads_nothing() {
    let folder = Folder::new();
    folder.write("a.pdf", &pdf(&["needle"]));
    let mut read = 0;
    let summary =
        search(&Options::new(&folder.0, "needle"), &AtomicBool::new(true), |event| read += usize::from(matches!(event, Event::Reading { .. })))
            .unwrap();
    assert_eq!(read, 0);
    assert_eq!(summary.stopped, Some(Stop::Cancelled));
}

#[test]
fn what_it_was_asked_to_search_is_checked_first() {
    let folder = Folder::new();
    let file = folder.write("a.pdf", &pdf(&["needle"]));
    let refused = |options: Options| search(&options, &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(refused(Options::new(&folder.0, "   ")).contains("empty"));
    assert!(refused(Options::new(&folder.0, "x".repeat(MAX_QUERY_CHARS + 1))).contains("longer"));
    assert!(refused(Options::new(&file, "needle")).contains("not a folder"));
    assert!(!refused(Options::new(folder.0.join("gone"), "needle")).is_empty());
    assert_eq!(run(&Options::new(&folder.0, "x".repeat(MAX_QUERY_CHARS))).summary.matches, 0, "the longest query taken");
}

#[test]
fn a_page_that_runs_out_of_time_ends_its_file_and_keeps_what_was_found() {
    let folder = Folder::new();
    let slow = folder.write("a.pdf", &slow_pdf("needle"));
    let next = folder.write("b.pdf", &pdf(&["needle"]));
    let mut options = Options::new(&folder.0, "needle");
    options.page_limit = PAGE_LIMIT;
    let found = run(&options);
    assert_eq!(reasons(&found, &slow), [&Skip::TooSlow]);
    assert_eq!(found.files[0].path, slow, "its first page was read in time");
    assert!(found.reading.contains(&next), "one slow file doesn't end the search");
    assert_eq!(found.summary.stopped, None);
}

#[test]
fn a_file_that_opens_in_time_lets_no_thread_go() {
    let left = none_left();
    let bytes = Arc::new(pdf(&["needle"]));
    assert!(matches!(super::open(&bytes, Duration::from_secs(60), &AtomicBool::new(false), &left), super::Opened::Pages(1)));
    assert_eq!(kept(&left), 0);
}

#[test]
fn a_file_or_a_page_out_of_time_leaves_its_thread_counted() {
    let left = none_left();
    let bytes = Arc::new(pdf(&["needle"]));
    assert!(matches!(super::open(&bytes, Duration::ZERO, &AtomicBool::new(false), &left), super::Opened::Skip(Skip::TooSlow)));
    assert_eq!(kept(&left), 1);
    let mut options = Options::new("unused", "needle");
    options.page_limit = PAGE_LIMIT;
    let read = super::read_pages(Arc::new(slow_pdf("needle")), 2, "needle", &options, 10, &AtomicBool::new(false), &left);
    assert_eq!(read.skip, Some(Skip::TooSlow));
    assert_eq!(kept(&left), 2);
}

#[test]
fn a_search_is_refused_while_too_many_threads_run_and_not_after() {
    let folder = Folder::new();
    folder.write("a.pdf", &pdf(&["needle"]));
    let options = Options::new(&folder.0, "needle");
    let left = none_left();
    let search = |left: &Mutex<Vec<Retiring>>| super::search_inner(&options, &AtomicBool::new(false), &mut |_| {}, left);
    let mut held: Vec<_> = (1..MAX_LEFT_RUNNING).map(|_| hold_a_thread(&left)).collect();
    assert!(search(&left).is_ok(), "one short of the limit");
    held.push(hold_a_thread(&left));
    let error = search(&left).unwrap_err();
    assert!(error.contains("still being read"), "{error}");
    drop(held);
    for _ in 0..5_000 {
        if super::running(&left) == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(search(&left).is_ok(), "they exited");
    assert_eq!(kept(&left), 0, "and were forgotten");
}

#[test]
fn a_file_that_takes_too_long_to_open_is_named() {
    let folder = Folder::new();
    let file = folder.write("a.pdf", &pdf(&["needle"]));
    let mut options = Options::new(&folder.0, "needle");
    options.open_limit = Duration::ZERO;
    let found = run(&options);
    assert_eq!(reasons(&found, &file), [&Skip::TooSlow]);
}

#[test]
fn too_many_slow_files_end_the_search() {
    let folder = Folder::new();
    for name in ["a.pdf", "b.pdf", "c.pdf", "d.pdf"] {
        folder.write(name, &pdf(&["needle"]));
    }
    let mut options = Options::new(&folder.0, "needle");
    options.open_limit = Duration::ZERO;
    let found = run(&options);
    assert_eq!(found.reading.len(), super::MAX_SLOW_FILES);
    assert_eq!(found.summary.stopped, Some(Stop::TooManySlowFiles));
}

#[test]
fn a_page_that_can_not_be_read_is_counted_and_what_was_read_is_kept() {
    // One page more than the file has: the renderer's answer for it is an error.
    let options = Options::new("unused", "needle");
    let left = none_left();
    let read = super::read_pages(Arc::new(pdf(&["one", "two needle"])), 3, "needle", &options, 10, &AtomicBool::new(false), &left);
    assert_eq!(read.skip, Some(Skip::PagesUnread(1)));
    assert_eq!(read.matches.len(), 1);
    assert_eq!(read.matches[0].page, 1);
    assert_eq!(kept(&left), 0, "every page was answered, so its worker is idle");
}

#[test]
fn a_file_with_a_page_unread_does_not_count_as_searched() {
    let options = Options::new("unused", "needle");
    let read = super::read_pages(Arc::new(pdf(&["one"])), 2, "needle", &options, 10, &AtomicBool::new(false), &none_left());
    assert_eq!(read.skip, Some(Skip::PagesUnread(1)));
    assert!(read.matches.is_empty());
}

#[test]
fn a_folder_that_can_not_be_listed_all_the_way_says_so() {
    let wide = Folder::new();
    for n in 0..=MAX_ENTRIES {
        fs::File::create(wide.0.join(format!("{n}.txt"))).unwrap();
    }
    assert!(run(&Options::new(&wide.0, "needle")).summary.folder_cut_short);

    let deep = Folder::new();
    let mut path = deep.0.clone();
    for _ in 0..=MAX_DEPTH {
        path.push("d");
    }
    fs::create_dir_all(&path).unwrap();
    assert!(run(&Options::new(&deep.0, "needle")).summary.folder_cut_short);
    let at_the_limit = Folder::new();
    let mut path = at_the_limit.0.clone();
    for _ in 0..MAX_DEPTH {
        path.push("d");
    }
    fs::create_dir_all(&path).unwrap();
    assert!(!run(&Options::new(&at_the_limit.0, "needle")).summary.folder_cut_short, "as deep as it goes, and no deeper");
}

#[cfg(unix)]
#[test]
fn a_subfolder_that_can_not_be_listed_is_named() {
    use std::os::unix::fs::PermissionsExt;
    let folder = Folder::new();
    folder.write("a.pdf", &pdf(&["needle"]));
    let locked = folder.0.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&locked).is_ok() {
        // Root reads everything.
        return;
    }
    let found = run(&Options::new(&folder.0, "needle"));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(reasons(&found, &locked)[..], [Skip::Unreadable(_)]), "{:?}", found.skipped);
    assert_eq!(found.summary.matches, 1, "and the rest is still searched");
}

#[test]
fn an_excerpt_stays_short_however_long_the_page_runs() {
    let folder = Folder::new();
    let line = format!("{} needle {}", "word ".repeat(12).trim(), "word ".repeat(12).trim());
    folder.write("a.pdf", &raw_pdf(&[format!("BT /F1 6 Tf 10 150 Td ({line}) Tj ET")]));
    let found = run(&Options::new(&folder.0, "needle"));
    let found = &found.files[0].matches[0];
    assert_eq!(found.text, "needle");
    for part in [&found.before, &found.after] {
        assert!((30..=36).contains(&part.chars().count()), "36 glyphs, spaces among them: {part:?}");
    }
}
