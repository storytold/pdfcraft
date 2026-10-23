//! `view` on a terminal, end to end: on a pseudo-terminal of its own that script(1) provides, so
//! the terminal running the tests is never touched. Keys go in through script's stdin; what
//! `view` draws comes out of its stdout. The window's size is set with stty; no pixel size is
//! reported, and the terminal these tests play doesn't know its cell size unless a test says so,
//! so cells are the 10 × 20 px `view` assumes.
#![cfg(unix)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// How long anything may take: generous, for a slow CI machine.
const PATIENCE: Duration = Duration::from_secs(60);

/// A US Letter document with `pages` pages.
fn fixture(test: &str, pages: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-view-pager-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let one = dir.join("one.pdf");
    let bytes = pdfcraft_engine::Session::new().create_from_text("Pager test", "Hello from PdfCraft").unwrap();
    std::fs::write(&one, bytes.as_slice()).unwrap();
    if pages == 1 {
        return one;
    }
    let many = dir.join("many.pdf");
    let mut args = vec!["combine".to_string()];
    args.extend(std::iter::repeat_n(one.to_str().unwrap().to_string(), pages));
    args.extend(["--out".to_string(), many.to_str().unwrap().to_string()]);
    assert!(Command::new(BIN).args(&args).status().unwrap().success());
    many
}

/// `line` run by a shell on a new pseudo-terminal: BSD script (macOS, FreeBSD) takes the command
/// after the output file; util-linux script (Linux) takes it as one shell line.
fn script(line: &str) -> Command {
    let mut script = Command::new("script");
    if cfg!(target_os = "linux") {
        script.args(["-q", "-e", "-c", line, "/dev/null"]);
    } else {
        script.args(["-q", "/dev/null", "sh", "-c", line]);
    }
    script
}

/// Whether script(1) gives a command a pseudo-terminal of the size asked for, which every test
/// here stands on. Where it can't (no script, or one that can't open a terminal), the tests skip
/// with a warning instead of failing over something that isn't `view`'s.
fn pty_works() -> bool {
    static WORKS: OnceLock<bool> = OnceLock::new();
    *WORKS.get_or_init(|| {
        let works = (|| {
            let mut child =
                script("stty rows 5 cols 7 && stty size").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
            let deadline = Instant::now() + Duration::from_secs(20);
            while child.try_wait().ok()?.is_none() {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let mut out = String::new();
            child.stdout.take()?.read_to_string(&mut out).ok()?;
            Some(out.contains("5 7"))
        })()
        .unwrap_or(false);
        if !works {
            eprintln!("warning: skipped: script(1) can't give a command a pseudo-terminal here");
        }
        works
    })
}

/// `view pdf args…` running on a `cols` × `rows` pseudo-terminal.
struct Session {
    child: Child,
    /// script's stdin: keys, and the terminal's answers to `view`'s questions.
    keys: Arc<Mutex<Option<ChildStdin>>>,
    output: Arc<Mutex<Vec<u8>>>,
    /// The files taken, as a terminal on this machine takes them, or what was wrong with one.
    taken: Arc<Mutex<Vec<Result<PathBuf, String>>>>,
}

/// The terminal at the other end of the pseudo-terminal: whether it reads the temporary files
/// `view` names, as kitty does (and deletes them), the temporary directory `view` gets, and the
/// cell size it answers when asked (the device attributes alone when `None`).
#[derive(Clone, Copy)]
struct Terminal<'a> {
    takes_files: bool,
    tmpdir: Option<&'a Path>,
    cell: Option<(u16, u16)>,
}

/// One that never reads files, as script(1) doesn't, and doesn't know its cell size.
const PLAIN: Terminal = Terminal { takes_files: false, tmpdir: None, cell: None };

/// `view`'s question about the cell size, which a pseudo-terminal without pixels gets.
const CELL_QUERY: &[u8] = b"\x1b[16t\x1b[c";

impl Session {
    fn start(pdf: &Path, args: &[&str], cols: u16, rows: u16) -> Self {
        Self::start_on(PLAIN, pdf, args, cols, rows)
    }

    fn start_on(terminal: Terminal, pdf: &Path, args: &[&str], cols: u16, rows: u16) -> Self {
        let quote = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
        let command = [BIN, "view", pdf.to_str().unwrap()].iter().chain(args).map(|a| quote(a)).collect::<Vec<_>>().join(" ");
        let mut script = script(&format!("stty rows {rows} cols {cols} && exec {command}"));
        if let Some(dir) = terminal.tmpdir {
            script.env("TMPDIR", dir);
        }
        // On this machine, whatever machine the tests run from: an SSH login would turn the files off.
        for name in ["TMUX", "TMUX_PANE", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"] {
            script.env_remove(name);
        }
        let mut child = script.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        let taken = Arc::new(Mutex::new(Vec::new()));
        let keys = Arc::new(Mutex::new(child.stdin.take()));
        let mut stdout = child.stdout.take().unwrap();
        let (collected, took, answers) = (output.clone(), taken.clone(), keys.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let (mut scanned, mut asked) = (0, 0);
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut output = collected.lock().unwrap();
                output.extend_from_slice(&buf[..n]);
                if terminal.takes_files {
                    scanned = take_files(&output, scanned, &mut took.lock().unwrap());
                }
                // Answer the cell size question as a terminal does, once per asking.
                while let Some(at) = output[asked..].windows(CELL_QUERY.len()).position(|w| w == CELL_QUERY) {
                    asked += at + CELL_QUERY.len();
                    let cell = terminal.cell.map(|(w, h)| format!("\x1b[6;{h};{w}t")).unwrap_or_default();
                    if let Some(stdin) = answers.lock().unwrap().as_mut() {
                        let _ = stdin.write_all(format!("{cell}\x1b[?62;c").as_bytes()).and_then(|()| stdin.flush());
                    }
                }
            }
        });
        Session { child, keys, output, taken }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
    }

    /// Wait until `done` holds for what was drawn so far; panics with the output if it never does.
    fn wait_until(&self, what: &str, done: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !done(&self.text()) {
            assert!(Instant::now() < deadline, "{what} never happened: {:?}", self.text());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn press(&mut self, keys: &str) {
        let mut stdin = self.keys.lock().unwrap();
        let stdin = stdin.as_mut().unwrap();
        stdin.write_all(keys.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }

    /// Wait for `view` to exit, all of its output collected.
    fn finish(&mut self) -> ExitStatus {
        let deadline = Instant::now() + PATIENCE;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "view didn't exit: {:?}", self.text());
            std::thread::sleep(Duration::from_millis(20));
        };
        *self.keys.lock().unwrap() = None;
        // The reader thread has the last of the output once script's stdout closes.
        std::thread::sleep(Duration::from_millis(100));
        status
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Take the files that the file transmissions in `output` from `from` on name, as kitty does:
/// read each, check it holds the pixels its escape code says, and delete it. Returns where the
/// next look starts (at a transmission not yet whole).
fn take_files(output: &[u8], mut from: usize, taken: &mut Vec<Result<PathBuf, String>>) -> usize {
    use base64::Engine as _;
    use std::os::unix::ffi::OsStrExt as _;
    const START: &[u8] = b"\x1b_Ga=t,f=32,t=t,";
    while let Some(at) = output[from..].windows(START.len()).position(|w| w == START).map(|p| from + p) {
        let Some(end) = output[at..].windows(2).position(|w| w == b"\x1b\\").map(|p| at + p) else { return at };
        from = end + 2;
        let code = String::from_utf8_lossy(&output[at + 3..end]).into_owned();
        let (control, payload) = code.split_once(';').unwrap_or((&code, ""));
        let key = |k: &str| control.split(',').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('=')?.parse::<u64>().ok());
        let path = base64::engine::general_purpose::STANDARD.decode(payload).map(|p| PathBuf::from(std::ffi::OsStr::from_bytes(&p)));
        taken.push(match (path, key("s"), key("v")) {
            (Ok(path), Some(w), Some(h)) => match std::fs::metadata(&path) {
                Ok(meta) if meta.len() == w * h * 4 => std::fs::remove_file(&path).map(|()| path).map_err(|e| e.to_string()),
                Ok(meta) => Err(format!("{}: {} bytes for {w} × {h} pixels", path.display(), meta.len())),
                Err(e) => Err(format!("{}: {e}", path.display())),
            },
            _ => Err(format!("unreadable: {code:?}")),
        });
    }
    output.len().saturating_sub(START.len()).max(from)
}

/// How many images were transmitted (for the pager) or shown at the cursor (when printing).
fn transmitted(text: &str) -> usize {
    text.matches("a=t,f=100,t=d,q=2,i=").count()
}

fn printed(text: &str) -> usize {
    text.matches("a=T,f=100,t=d,q=2,").count()
}

#[test]
fn more_than_a_page_opens_the_pager_and_pages_arrive_without_a_key() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("tall", 3);
    // A tall window: one page to the screen. No --pager: three pages on a terminal page.
    let mut view = Session::start(&pdf, &["--protocol", "kitty"], 80, 60);
    // No key yet: the first page arrives from the render thread on its own, its status line
    // replacing "rendering". Before, the pager waited on a read until a key came.
    view.wait_until("page 1 shown", |t| t.contains("q quit") && t.contains(" page 1/3 "));
    let text = view.text();
    assert!(text.contains("\x1b[?1049h"), "the alternate screen");
    assert!(text.contains(",p=1,C=1,"), "placed without moving the cursor");

    // The next pages are rendered and sent ahead, still without a key.
    view.wait_until("pages 2 and 3 sent ahead", |t| transmitted(t) == 3);
    // So turning the page sends nothing: page 2 is only placed, and page 1 hidden, at once.
    view.press("n");
    view.wait_until("page 2 shown", |t| t.contains(" page 2/3 ") && t.contains("a=d,d=i,q=2,i="));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(transmitted(&view.text()), 3, "a page was sent again");

    // q quits and puts the screen back.
    view.press("q");
    assert!(view.finish().success());
    assert!(view.text().contains("\x1b[?1049l"), "the alternate screen was left");
}

#[test]
fn a_wide_window_shows_two_pages_and_turns_two_at_a_time() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("wide", 3);
    let mut view = Session::start(&pdf, &["--protocol", "kitty"], 120, 24);
    view.wait_until("pages 1-2 shown", |t| t.contains(" pages 1–2/3 ") && t.contains("q quit"));
    // Page 3, on the next spread, is sent ahead: three images, three ids.
    view.wait_until("page 3 sent ahead", |t| transmitted(t) == 3);
    let text = view.text();
    let ids: std::collections::HashSet<String> = text.match_indices("a=t,f=100,t=d,q=2,i=").map(|(at, _)| view_id(&text[at..])).collect();
    assert_eq!(ids.len(), 3, "{ids:?}");
    // Space turns the spread, page 3 alone on the left, with nothing sent.
    view.press(" ");
    view.wait_until("page 3 shown", |t| t.contains(" page 3/3 "));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(transmitted(&view.text()), 3, "a page was sent again");
    // d: one page to the screen, from where the reader is; p goes back one page. One page to the
    // screen is wider, so pages are made again for it.
    view.press("dp");
    view.wait_until("page 2 alone", |t| t.contains(" page 2/3 ") && transmitted(t) > 3);
    view.press("q");
    assert!(view.finish().success());
}

#[test]
fn a_page_given_twice_shows_twice_side_by_side() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("twice", 3);
    let mut view = Session::start(&pdf, &["--protocol", "kitty", "--pages", "1,1"], 120, 24);
    view.wait_until("page 1 twice", |t| t.contains(" pages 1–1/3 ") && t.contains("q quit"));
    // One image, placed twice: each place on screen a placement of its own.
    let text = view.text();
    assert_eq!(transmitted(&text), 1, "{text:?}");
    let id = view_id(&text[text.find("a=t,f=100,t=d,q=2,i=").unwrap()..]);
    for placement in [1, 2] {
        assert!(text.contains(&format!("a=p,q=1,i={id},p={placement},")), "placement {placement}: {text:?}");
    }
    view.press("q");
    assert!(view.finish().success());
}

#[test]
fn an_image_the_terminal_dropped_is_sent_again_and_its_answers_are_never_keys() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("dropped", 3);
    let mut view = Session::start(&pdf, &["--protocol", "kitty"], 80, 60);
    view.wait_until("pages 1-3 sent", |t| t.contains(" page 1/3 ") && t.contains("q quit") && transmitted(t) == 3);
    let text = view.text();
    let at = text.find("a=p,q=1,i=").unwrap();
    let id: String = text[at + "a=p,q=1,i=".len()..].chars().take_while(char::is_ascii_digit).collect();
    // Answers that say nothing: an OK, another error, one cut in two. Not one of them is a key:
    // their ESC isn't Esc (which quits), nor their letters keys.
    view.press(&format!("\x1b_Gi={id};OK\x1b\\\x1b_Gi={id};EINVAL:quit\x1b\\\x1b_Gi={id};O"));
    std::thread::sleep(Duration::from_millis(100));
    view.press("K\x1b\\");
    std::thread::sleep(Duration::from_millis(300));
    assert!(view.child.try_wait().unwrap().is_none(), "an answer quit the pager");
    assert_eq!(transmitted(&view.text()), 3, "an answer that says nothing sent a page");
    // kitty dropped page 1 to stay under its quota, and answers its placement so: it is sent
    // again, and placed.
    view.press(&format!("\x1b_Gi={id},p=1;ENOENT:Put command refers to non-existent image with id: {id} and number: 0\x1b\\"));
    view.wait_until("page 1 sent again", |t| transmitted(t) == 4);
    let again = view.text();
    let new = view_id(&again[again.rfind("a=t,f=100,t=d,q=2,i=").unwrap()..]);
    assert_ne!(new, id);
    view.wait_until("page 1 placed again", |t| t.contains(&format!("a=p,q=1,i={new},p=1,")));
    view.press("q");
    assert!(view.finish().success());
}

#[test]
fn a_reply_cut_in_two_is_never_keys_and_esc_still_quits() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("cut", 3);
    let mut view = Session::start(&pdf, &["--protocol", "kitty"], 80, 60);
    // Settled: page 1 shown, and every page sent.
    view.wait_until("pages 1-3 sent", |t| t.contains(" page 1/3 ") && t.contains("q quit") && transmitted(t) == 3);
    // A late cell size and device attributes, as over a slow connection: each cut past its CSI
    // prefix, the rest coming later than an Esc waits for more. Neither part is a key: the ESC
    // isn't Esc (which quits), nor are the digits a page number.
    for (first, rest) in [("\x1b[6;", "25;14t"), ("\x1b[", "?62;c")] {
        view.press(first);
        std::thread::sleep(Duration::from_millis(250));
        assert!(view.child.try_wait().unwrap().is_none(), "{first:?} quit the pager");
        view.press(rest);
        std::thread::sleep(Duration::from_millis(250));
        assert!(view.child.try_wait().unwrap().is_none(), "{first:?} then {rest:?} quit the pager");
    }
    assert!(!view.text().contains("go to page"), "a reply typed a page number");
    // A key cut in two the same way is the key: Page Down.
    view.press("\x1b[6");
    std::thread::sleep(Duration::from_millis(250));
    view.press("~");
    view.wait_until("page 2 shown", |t| t.contains(" page 2/3 "));
    // Esc on its own still quits.
    view.press("\x1b");
    assert!(view.finish().success());
    assert!(view.text().contains("\x1b[?1049l"), "the alternate screen was left");
}

/// The `i=` value at the start of a transmission.
fn view_id(transmission: &str) -> String {
    transmission.trim_start_matches("a=t,f=100,t=d,q=2,i=").chars().take_while(char::is_ascii_digit).collect()
}

/// The first page placed: the cells it covers, and its size in pixels, from its PNG header. (The
/// first page sent may be one rendered ahead, never placed yet.)
fn first_page_size(text: &str) -> ((u32, u32), (u32, u32)) {
    use base64::Engine as _;
    let placed = &text[text.find("a=p,q=1,i=").unwrap()..];
    let control = placed.split(['\x1b', ';']).next().unwrap();
    let key = |k: &str| control.split(',').find_map(|kv| kv.strip_prefix(k)).unwrap().parse::<u32>().unwrap();
    let sent = &text[text.find(&format!("a=t,f=100,t=d,q=2,i={},", key("i="))).unwrap()..];
    let payload = &sent[sent.find(';').unwrap() + 1..];
    let header = base64::engine::general_purpose::STANDARD.decode(&payload[..44]).unwrap();
    let be = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().unwrap());
    ((be(16), be(20)), (key("c="), key("r=")))
}

#[test]
fn a_terminal_without_pixels_is_asked_its_cell_size() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("cells", 3);
    // As under `docker exec`: the pseudo-terminal has no pixel size, and the terminal knows its
    // cells are 16 × 24 px. One that doesn't know gets the 10 × 20 px assumed.
    for (terminal, (cell_w, cell_h)) in [(Terminal { cell: Some((16, 24)), ..PLAIN }, (16, 24)), (PLAIN, (10, 20))] {
        let mut view = Session::start_on(terminal, &pdf, &["--protocol", "kitty"], 80, 60);
        view.wait_until("page 1 shown", |t| t.contains(" page 1/3 ") && t.contains("q quit"));
        let text = view.text();
        assert_eq!(text.matches("\x1b[16t").count(), 1, "asked once");
        // Rendered for those cells: the page fills the last of the cells it covers.
        let ((w, h), (cols, rows)) = first_page_size(&text);
        assert!(w <= cols * cell_w + 1 && w + 1 > (cols - 1) * cell_w, "{w} px wide over {cols} cells {cell_w} px wide");
        assert!(h <= rows * cell_h + 1 && h + 1 > (rows - 1) * cell_h, "{h} px tall over {rows} cells {cell_h} px tall");
        view.press("q");
        assert!(view.finish().success());
    }
}

#[test]
fn the_first_key_after_the_cell_size_question_reaches_the_pager() {
    if !pty_works() {
        return;
    }
    // On FreeBSD a read begun in canonical mode stays a line read after raw mode returns. Were
    // the terminal back in canonical mode between the question and the pager, a reader whose next
    // read began in that gap would hold every key until Enter: `q` would never quit. Whether a
    // read begins in the gap is a race, which many sessions at once, each asked its cell size
    // (no pixel size here), make likely to show.
    let pdf = fixture("first-key", 2);
    let sessions: Vec<Session> = (0..16).map(|_| Session::start(&pdf, &["--protocol", "kitty"], 80, 60)).collect();
    for mut view in sessions {
        view.wait_until("page 1 shown", |t| t.contains(" page 1/2 ") && t.contains("q quit"));
        assert_eq!(view.text().matches("\x1b[16t").count(), 1, "asked its cell size");
        view.press("q");
        assert!(view.finish().success());
    }
}

#[test]
fn one_page_is_printed_without_the_pager() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("one", 1);
    let mut view = Session::start(&pdf, &["--protocol", "kitty"], 80, 24);
    assert!(view.finish().success());
    let text = view.text();
    assert_eq!(printed(&text), 1, "{text:?}");
    assert!(!text.contains("\x1b[?1049h"), "no alternate screen");
}

#[test]
fn no_pager_prints_every_page() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("print", 3);
    let mut view = Session::start(&pdf, &["--protocol", "kitty", "--no-pager"], 80, 24);
    assert!(view.finish().success());
    let text = view.text();
    assert_eq!(printed(&text), 3, "{text:?}");
    assert!(!text.contains("\x1b[?1049h"), "no alternate screen");
}

#[test]
fn pager_shows_even_one_page() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("forced", 1);
    let mut view = Session::start(&pdf, &["--protocol", "kitty", "--pager"], 80, 60);
    view.wait_until("page 1 shown", |t| t.contains(" page 1/1 ") && transmitted(t) == 1);
    view.press("q");
    assert!(view.finish().success());
}

/// A directory of the test's own, empty, for `view`'s temporary files. Owner-only whatever the
/// umask: `view` rightly refuses a parent that others can write to without the sticky bit, which
/// a umask of 0002 (common on Linux) would make it.
fn tmpdir(test: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-view-tmp-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// The files `view` left in the temporary directory `dir`: in its own directory there.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_in(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn a_terminal_that_takes_files_gets_its_pages_through_them() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("files", 3);
    let dir = tmpdir("files");
    let terminal = Terminal { takes_files: true, tmpdir: Some(&dir), cell: None };
    let mut view = Session::start_on(terminal, &pdf, &["--protocol", "kitty"], 80, 60);
    view.wait_until("page 1 shown", |t| t.contains("q quit") && t.contains(" page 1/3 "));
    // The probe first: one pixel, its image deleted straight after.
    view.wait_until("the probe taken", |t| t.contains("a=t,f=32,t=t,s=1,v=1,q=2,i="));
    assert!(view.taken.lock().unwrap().first().is_some_and(Result::is_ok), "{:?}", view.taken.lock().unwrap());
    // r sends every page again, long after the probe: each through a file, none as a PNG.
    std::thread::sleep(Duration::from_millis(300));
    let before = view.text().len();
    view.press("r");
    let after = |t: &str| t.get(before..).unwrap_or_default().to_string();
    view.wait_until("pages 1-3 sent again", |t| after(t).matches("a=t,f=32,t=t,").count() == 3);
    std::thread::sleep(Duration::from_millis(200));
    let text = view.text();
    assert_eq!(transmitted(&after(&text)), 0, "a PNG after the probe was taken");
    let taken = view.taken.lock().unwrap().clone();
    assert!(taken.iter().all(Result::is_ok), "{taken:?}");
    assert!(taken.len() >= 4, "{taken:?}");
    for path in taken.iter().flatten() {
        assert!(path.starts_with(&dir) && path.to_string_lossy().contains("tty-graphics-protocol"), "{path:?}");
    }
    view.press("q");
    assert!(view.finish().success());
    assert!(files_in(&dir).is_empty(), "left behind: {:?}", files_in(&dir));
}

#[test]
fn a_terminal_that_leaves_files_gets_pngs_and_no_file_stays() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("nofiles", 3);
    let dir = tmpdir("nofiles");
    let terminal = Terminal { takes_files: false, tmpdir: Some(&dir), cell: None };
    let mut view = Session::start_on(terminal, &pdf, &["--protocol", "kitty"], 80, 60);
    view.wait_until("pages 1-3 sent as PNGs", |t| t.contains(" page 1/3 ") && transmitted(t) == 3);
    let text = view.text();
    assert_eq!(text.matches("a=t,f=32,t=t,").count(), 1, "only the probe went through a file");
    // The probe waits for the terminal a while, and goes after that or when the pager ends.
    view.press("q");
    assert!(view.finish().success());
    assert!(files_in(&dir).is_empty(), "left behind: {:?}", files_in(&dir));
}

#[test]
fn compression_by_name_sends_pngs_without_trying_files() {
    if !pty_works() {
        return;
    }
    let pdf = fixture("named", 3);
    let dir = tmpdir("named");
    let terminal = Terminal { takes_files: true, tmpdir: Some(&dir), cell: None };
    let mut view = Session::start_on(terminal, &pdf, &["--protocol", "kitty", "--compression", "fast"], 80, 60);
    view.wait_until("pages 1-3 sent as PNGs", |t| t.contains(" page 1/3 ") && transmitted(t) == 3);
    view.press("q");
    assert!(view.finish().success());
    assert!(!view.text().contains("t=t"), "no file, not even the probe");
}
