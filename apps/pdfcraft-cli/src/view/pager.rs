//! `view --pager`: one page at a time on the alternate screen, like `less`, or two side by side
//! when the window is wide enough, turning two at a time like a book's.
//!
//! [`Nav`] is where the reader is and what each key does, with no IO. [`Pager`] renders and
//! draws what `Nav` says, and [`Screen`] puts the terminal back however the pager ends.
//!
//! Turning a page should be instant, so the pager works ahead. The pages on screen render first,
//! then the next screens and the one before, on worker threads (keys keep working meanwhile, and
//! the renderer's watchdog gives up on a page that takes too long). Each page is sent to the
//! terminal as soon as it is ready, under an image id of its own, and stays there while it is
//! near: turning to it only places it. The pages on screen stay until the next ones are ready,
//! so the screen never goes blank in between.
//!
//! A terminal on this machine is handed pages through temporary files when it takes them
//! (`files.rs`): only a path goes through the stream, so a page turn never waits behind the
//! megabytes of pages sent ahead, and neither side spends time on PNG.
//!
//! What the terminal keeps can still go: kitty drops images past its storage quota, and tmux
//! throws away what it can't deliver. The pages kept stay well under the quota ([`BUDGET`]); a
//! placement of an image the terminal no longer has is answered, and the page is sent again; and
//! inside tmux, the pager asks tmux after writing whether everything reached the terminal.
//! `README.md` beside this file has the details.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use pdfcraft_engine::export::PngEffort;
use pdfcraft_render::{RenderPool, RenderedPage};

use super::files::{Files, Written};
use super::fit::{self, Cell, FALLBACK_CELL, Fit, Layout};
use super::keys::{self, Key};
use super::kitty::{self, Rect};
use super::tty::{RawMode, Tty, WindowSize};
use super::{Doc, FALLBACK_SIZE, Output, QUIET, Terminal, checked, png, printable, request, tmux};

/// How often the window size is checked while no key comes.
const TICK: Duration = Duration::from_millis(200);
/// How often renders in progress are checked on.
const POLL: Duration = Duration::from_millis(10);
/// How long a lone ESC waits for more before it counts as the Esc key. A longer escape sequence
/// cut off waits for the rest up to [`PENDING_LIMIT`], and is never a key.
const ESCAPE_WAIT: Duration = Duration::from_millis(50);
/// Rows `j`, `k`, the arrows and Enter scroll.
const STEP: u16 = 3;
/// The most digits a typed page number keeps.
const MAX_TYPED: usize = 9;
/// Bytes kept from an escape sequence or a terminal's answer still arriving; past this many, or
/// [`PENDING_LIMIT`], they are dropped.
const MAX_PENDING: usize = keys::MAX_ANSWER * 2;
const PENDING_LIMIT: Duration = Duration::from_secs(1);
/// Screens rendered and sent ahead of the one on screen, and behind it.
const AHEAD: usize = 2;
const BEHIND: usize = 1;
/// Pages kept in the terminal, ready to show at once: the most recently shown stay.
const MAX_SENT: usize = 12;
/// The most bytes of pixels kept in the terminal. kitty keeps 320 MB of images per screen, and
/// past that drops nearly every image not on screen at once; inside tmux, other panes' images
/// count against the same 320 MB.
const BUDGET: u64 = 192 << 20;
/// The least the budget comes down to when the terminal drops images all the same.
const MIN_BUDGET: u64 = 32 << 20;
/// Failures remembered, so a page that fails isn't rendered again and again.
const MAX_FAILED: usize = 16;
/// Render threads: one can work ahead while the other renders what the reader asked for.
const THREADS: usize = 2;
/// Inside tmux, how long after the last write to ask tmux whether it reached the terminal.
const TMUX_AFTER_WRITE: Duration = Duration::from_millis(150);
/// Inside tmux, how often to ask anyway: whether the pane shows again, or another terminal came.
const TMUX_EVERY: Duration = Duration::from_secs(1);
/// How many times the pager sends its pages again after tmux or the terminal threw some away,
/// before it stops trying by itself (`r` still does).
const MAX_RESENDS: u32 = 8;
/// How long the pager waits at the end for the terminal's answers to its last placements, so
/// none of them lands on the command line.
const ANSWER_WAIT: Duration = Duration::from_millis(100);
/// The reminder of the keys on the status line, shorter and shorter for a narrow window.
const KEYS: [&str; 3] = ["space b j k · n p · g G · 12⏎ · f fit · d two-up · q quit", "n p · f fit · d two-up · q quit", "n p · q quit"];

/// Page through `pages` of `doc` until the user quits. Image ids are taken in turn from
/// `first_id` on ([`super::kitty::IdColor::id`]).
pub(super) fn run(tty: &Tty, doc: &Doc, name: &str, terminal: &Terminal, pages: Vec<usize>, fit: Fit, first_id: u64) -> Result<(), String> {
    // Inside tmux: who shows the pane, before any image goes.
    let mut watch = None;
    let mut showing = 1;
    if let Output::Tmux(_) = terminal.output
        && let Some(report) = tmux::report()
    {
        let mut w = tmux::Watch::default();
        showing = w.update(&report, 0).showing;
        watch = Some(w);
    }
    let mut pager = Pager {
        screen: Screen::enter(tty)?,
        tty,
        doc,
        pool: doc.pool(THREADS),
        name,
        output: terminal.output,
        cell: terminal.cell,
        effort: terminal.effort,
        nav: Nav::new(pages, doc.count, fit),
        sent: Vec::new(),
        wanted: Vec::new(),
        failed: Vec::new(),
        on_screen: [None, None],
        placed: Vec::new(),
        size: None,
        first_id,
        next_id: 0,
        tag: 0,
        tmux_check: watch.as_ref().map(|_| Instant::now() + TMUX_EVERY),
        watch,
        hidden: false,
        transmitted: 0,
        resends: 0,
        budget: BUDGET,
        files: None,
        stalled_files: None,
        placed_at: None,
    };
    // Files only where one terminal gets the escape codes: inside tmux, every terminal showing
    // the pane gets them, and the first to read a file deletes it before the others can. (Inside
    // a tmux that doesn't say, the pager can't know: no files.)
    if terminal.files && showing == 1 && (terminal.output == Output::Direct || pager.watch.is_some()) {
        let probe_id = pager.new_id();
        if let Some((files, probe)) = Files::start(probe_id, pager.output.wrap()) {
            pager.wrote(1);
            tty.write_all(&probe)?;
            pager.files = Some(files);
        }
    }
    pager.draw()?;
    let mut size = tty.size();
    let mut pending: Vec<u8> = Vec::new();
    let mut pending_since = Instant::now();
    loop {
        let wait = if !pending.is_empty() {
            ESCAPE_WAIT
        } else if !pager.wanted.is_empty() && !pager.hidden {
            POLL
        } else {
            TICK
        };
        let wait = pager.tmux_check.map_or(wait, |at| wait.min(at.saturating_duration_since(Instant::now())));
        let read = tty.read_timeout(wait)?;
        if let Some(bytes) = &read {
            if pending.is_empty() {
                pending_since = Instant::now();
            }
            pending.extend_from_slice(bytes);
        }
        let mut redraw = false;
        if !pending.is_empty() {
            // When nothing more arrived, a lone ESC was the Esc key.
            let (keys, used) = keys::decode(&pending, read.is_none());
            pending.drain(..used.min(pending.len()));
            // What is left is the start of something cut off: it gets a while to complete.
            if used > 0 {
                pending_since = Instant::now();
            }
            if pending.len() > MAX_PENDING || pending_since.elapsed() > PENDING_LIMIT {
                pending.clear();
            }
            let mut quit = false;
            for key in keys {
                if let Key::Missing(id) = key {
                    redraw |= pager.missing(id);
                } else if pager.nav.key(key) == Step::Quit {
                    quit = true;
                    break;
                } else {
                    redraw = true;
                }
            }
            if quit {
                pager.await_answers();
                break;
            }
        } else if read.is_none() {
            // Nothing typed: redraw if the window changed size.
            let now = tty.size();
            redraw = now != size;
            size = now;
        }
        let lost = pager.lost_files()?;
        // Nothing goes through tmux while the pane is hidden: it would be thrown away.
        let arrived = !pager.hidden && pager.receive()?;
        if arrived || redraw || lost {
            pager.draw()?;
        }
        if pager.tmux_check.is_some_and(|at| Instant::now() >= at) && pager.ask_tmux() {
            pager.draw()?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Continue,
    Quit,
}

/// Where the reader is, and what keys do to it.
#[derive(Debug)]
struct Nav {
    /// The pages to page through, in order: `--pages`, or the whole document.
    pages: Vec<usize>,
    /// Where in `pages` the reader is.
    at: usize,
    /// Pages in the document.
    count: usize,
    fit: Fit,
    /// Two pages side by side when they fit (`d` turns it off and on).
    two_up: bool,
    /// From the last draw: pages on screen (1 or 2), the image rows on screen, and the most
    /// `scroll` can be.
    per_screen: usize,
    visible: u16,
    max_scroll: u16,
    /// Image rows scrolled off the top. Past the bottom until the next draw clamps it.
    scroll: u16,
    /// A page number being typed.
    typed: String,
    /// Shown in the status line until the next key.
    message: Option<String>,
    /// Render the pages again even if nothing changed (`r`, Ctrl-L).
    rerender: bool,
}

impl Nav {
    fn new(pages: Vec<usize>, count: usize, fit: Fit) -> Self {
        Self {
            pages,
            at: 0,
            count,
            fit,
            two_up: true,
            per_screen: 1,
            visible: 1,
            max_scroll: 0,
            scroll: 0,
            typed: String::new(),
            message: None,
            rerender: false,
        }
    }

    /// Where in `pages` the screen starts. Two pages side by side are always an even place and
    /// the odd one after it, the way a book's spreads pair them.
    fn first(&self) -> usize {
        if self.per_screen == 2 { self.at - self.at % 2 } else { self.at }
    }

    /// The pages on screen: the one alone, or the two side by side (the second missing past the
    /// end).
    fn on_screen(&self) -> [Option<usize>; 2] {
        let first = self.first();
        let second = if self.per_screen == 2 { self.pages.get(first.saturating_add(1)).copied() } else { None };
        [self.pages.get(first).copied(), second]
    }

    /// The pages worth having ready around the screen, nearest first: the next `ahead` screens,
    /// then the `behind` screens before.
    fn around(&self, ahead: usize, behind: usize) -> Vec<usize> {
        let per = self.per_screen.max(1);
        let first = self.first();
        let screen = |start: usize| (start..start.saturating_add(per)).filter_map(|i| self.pages.get(i).copied());
        // Both stop at the ends of the list, however many screens are asked for.
        let next = (1..=ahead).map(|k| first.saturating_add(k.saturating_mul(per))).take_while(|start| *start < self.pages.len()).flat_map(screen);
        let before = (1..=behind).map_while(|k| first.checked_sub(k.saturating_mul(per))).flat_map(screen);
        next.chain(before).collect()
    }

    fn key(&mut self, key: Key) -> Step {
        self.message = None;
        let typing = !self.typed.is_empty();
        match key {
            Key::Char('q') | Key::Interrupt => return Step::Quit,
            Key::Escape if !typing => return Step::Quit,
            Key::Escape => self.typed.clear(),
            // Past MAX_TYPED digits, more are ignored (no other arm takes a digit).
            Key::Char(c @ '0'..='9') if self.typed.len() < MAX_TYPED => self.typed.push(c),
            Key::Backspace => {
                self.typed.pop();
            }
            Key::Enter | Key::Char('g') if typing => self.go_typed(),
            Key::Char(' ') | Key::PageDown => self.forward(self.visible.max(1)),
            Key::Char('b') | Key::PageUp => self.back(self.visible.max(1)),
            Key::Char('j') | Key::Down | Key::Enter => self.forward(STEP),
            Key::Char('k') | Key::Up => self.back(STEP),
            Key::Char('n') | Key::Right => self.go(self.first().saturating_add(self.per_screen)),
            Key::Char('p') | Key::Left => self.go(self.first().saturating_sub(self.per_screen)),
            Key::Char('g') | Key::Home => self.go(0),
            Key::Char('G') | Key::End => self.go(usize::MAX),
            // One page to the screen from now on: keys before the next draw step by one.
            Key::Char('f') => {
                self.fit = self.fit.toggled();
                self.scroll = 0;
                if self.fit == Fit::Width {
                    self.per_screen = 1;
                }
            }
            Key::Char('d') => {
                self.two_up = !self.two_up;
                self.scroll = 0;
                if !self.two_up {
                    self.per_screen = 1;
                }
            }
            Key::Char('r' | '\u{c}') => self.rerender = true,
            _ => {}
        }
        Step::Continue
    }

    /// Down `rows`; from the bottom of the screen's pages, on to the top of the next.
    fn forward(&mut self, rows: u16) {
        let next = self.first().saturating_add(self.per_screen);
        if self.scroll < self.max_scroll {
            self.scroll = self.scroll.saturating_add(rows).min(self.max_scroll);
        } else if next < self.pages.len() {
            self.at = next;
            self.scroll = 0;
        }
    }

    /// Up `rows`; from the top of the screen's pages, back to the bottom of the ones before.
    fn back(&mut self, rows: u16) {
        let first = self.first();
        if self.scroll > 0 {
            self.scroll = self.scroll.saturating_sub(rows);
        } else if first > 0 {
            self.at = first.saturating_sub(self.per_screen);
            self.scroll = u16::MAX;
        }
    }

    /// To the top of the page at `index` in `pages`, or of the last one when past it.
    fn go(&mut self, index: usize) {
        self.at = index.min(self.pages.len().saturating_sub(1));
        self.scroll = 0;
    }

    /// To the page whose number was typed, if it is one being paged through.
    fn go_typed(&mut self) {
        let wanted = self.typed.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
        match wanted.map(|page| (page, self.pages.iter().position(|p| *p == page))) {
            Some((_, Some(index))) => self.go(index),
            Some((page, None)) if page < self.count => self.message = Some(format!("page {} isn't one of the pages given", self.typed)),
            _ => self.message = Some(format!("no page {} (1–{})", self.typed, self.count)),
        }
        self.typed.clear();
    }

    /// After a draw: `per_screen` pages are on screen, `visible` of the first's `rows` image rows.
    /// Keeps `scroll` in range.
    fn shown(&mut self, per_screen: usize, visible: u16, rows: u16) {
        self.per_screen = per_screen;
        self.visible = visible;
        self.max_scroll = rows.saturating_sub(visible);
        self.scroll = self.scroll.min(self.max_scroll);
    }

    /// The status line for a window `cols` wide: exactly that many characters, none of them a
    /// control character (the file name and the messages may come from anywhere). The right side
    /// keeps two spaces from the left: the reminder of the keys gets shorter, then goes, in a
    /// narrow window, and a message cuts the file name short instead.
    fn status(&self, name: &str, cols: u16, rendering: bool) -> String {
        let said = match (&self.message, self.typed.is_empty(), rendering) {
            (Some(message), _, _) => Some(message.clone()),
            (None, false, _) => Some(format!("go to page {}", self.typed)),
            (None, true, true) => Some("rendering…".into()),
            (None, true, false) => None,
        };
        let pages = match self.on_screen() {
            [Some(a), Some(b)] => format!("pages {}–{}/{}", a.saturating_add(1), b.saturating_add(1), self.count),
            [Some(a), None] => format!("page {}/{}", a.saturating_add(1), self.count),
            _ => "no page".into(),
        };
        let mut place = format!(" · {pages} · fit {} ", self.fit.name());
        if self.pages.len() != self.count {
            place.push_str(&format!("· {} of {} ", self.first().saturating_add(1), self.pages.len()));
        }
        let width = usize::from(cols);
        let chars = |text: &str| text.chars().count();
        // Both sides, with a space before the name, one at the end, and two between the sides
        // (`place` ends with one of them).
        let fits = |name: &str, right: &str| chars(name).saturating_add(chars(&place)).saturating_add(chars(right)).saturating_add(3) <= width;
        let right = said.unwrap_or_else(|| KEYS.iter().find(|keys| fits(name, keys)).map_or("", |keys| keys).to_owned());
        // The file name gives way to the rest.
        let around = chars(&place).saturating_add(chars(&right)).saturating_add(if right.is_empty() { 2 } else { 3 });
        let room = width.saturating_sub(around);
        let name: String = if chars(name) <= room { name.to_owned() } else { name.chars().take(room.saturating_sub(1)).chain(['…']).collect() };
        let left = format!(" {name}{place}");
        let gap = width.saturating_sub(chars(&left).saturating_add(chars(&right)).saturating_add(1));
        let line = printable(&format!("{left}{}{right} ", " ".repeat(gap)));
        line.chars().chain(std::iter::repeat(' ')).take(width).collect()
    }
}

/// The alternate screen, raw mode, no cursor and no line wrap, all undone when this drops (on
/// return, on an error through `?` and while a panic unwinds).
struct Screen<'a> {
    tty: &'a Tty,
    _raw: RawMode<'a>,
}

impl<'a> Screen<'a> {
    fn enter(tty: &'a Tty) -> Result<Self, String> {
        // The guard comes first, so a write that fails halfway is undone too.
        let screen = Self { tty, _raw: tty.raw()? };
        QUIET.store(true, Ordering::Relaxed);
        tty.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?7l")?;
        Ok(screen)
    }
}

impl Drop for Screen<'_> {
    fn drop(&mut self) {
        // End synchronized output and reset the colours too, in case a redraw stopped halfway.
        let _ = self.tty.write_all(b"\x1b[?2026l\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l");
        QUIET.store(false, Ordering::Relaxed);
    }
}

/// A page as drawn: which page, fitted how, into how many columns of what window.
#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    page: usize,
    fit: Fit,
    cols: u16,
    size: WindowSize,
}

/// A page rendered for the window and sent to the terminal under an image id of its own.
struct Sent {
    view: View,
    id: u32,
    /// The cells it covers and its size in pixels.
    cols: u16,
    rows: u16,
    width: u32,
    height: u32,
    /// What the reader should know about it while it is on screen (content skipped).
    warning: Option<String>,
}

impl Sent {
    /// What it takes up in the terminal: its pixels, four bytes each.
    fn bytes(&self) -> u64 {
        u64::from(self.width).saturating_mul(u64::from(self.height)).saturating_mul(4)
    }
}

/// A render asked for and not back yet.
struct Wanted {
    view: View,
    layout: Layout,
    tag: u64,
}

struct Pager<'a> {
    /// Dropped after the pager's own `drop` has deleted its images.
    screen: Screen<'a>,
    tty: &'a Tty,
    doc: &'a Doc,
    pool: RenderPool,
    name: &'a str,
    output: Output,
    /// The cell size tmux or the terminal reports, for a tty that reports no pixel size.
    cell: Option<Cell>,
    effort: PngEffort,
    nav: Nav,
    /// Pages in the terminal, ready to place, the most recently shown last.
    sent: Vec<Sent>,
    /// Renders asked for, the most urgent first: the pages on screen, then the ones around them.
    wanted: Vec<Wanted>,
    /// Views that failed and why: not asked for again until the window changes or `r`.
    failed: Vec<(View, String)>,
    /// The views the reader wants on screen, from the last draw.
    on_screen: [Option<View>; 2],
    /// The images placed on screen by the last full redraw, and under which placement.
    placed: Vec<(u32, u32)>,
    /// The window the pages in `sent` were made for.
    size: Option<WindowSize>,
    first_id: u64,
    next_id: u64,
    tag: u64,
    /// Inside tmux: which terminals show the pane and what they missed, and when to ask again.
    watch: Option<tmux::Watch>,
    tmux_check: Option<Instant>,
    /// Inside tmux: no terminal shows the pane, so nothing is sent (tmux would throw it away).
    hidden: bool,
    /// Images sent so far.
    transmitted: u64,
    /// Times the pages were sent again because tmux or the terminal threw some away.
    resends: u32,
    /// The most bytes of pages kept in the terminal: [`BUDGET`], less once it drops images.
    budget: u64,
    /// Temporary files, while the terminal may take pages through them.
    files: Option<Files>,
    /// Temporary files whose thread stopped answering, kept to the end: dropping them waits for
    /// the thread to remove its files, which is only worth it once the terminal is put back
    /// (`screen` drops first).
    stalled_files: Option<Files>,
    /// When the last placements went out: their errors are answered.
    placed_at: Option<Instant>,
}

/// A page ready to send.
enum Payload {
    Png(Vec<u8>),
    /// Straight RGBA pixels in a temporary file.
    File(PathBuf),
}

impl Drop for Pager<'_> {
    fn drop(&mut self) {
        let mut out = Vec::new();
        for sent in &self.sent {
            kitty::delete(&mut out, sent.id, self.output.wrap());
        }
        let _ = self.screen.tty.write_all(&out);
    }
}

impl Pager<'_> {
    /// Redraw: decide what is on screen and what to work on ahead, then place the pages on
    /// screen once they are all in the terminal (until then the pages before stay up), and write
    /// the status line.
    fn draw(&mut self) -> Result<(), String> {
        let size = self.tty.size().unwrap_or(FALLBACK_SIZE);
        let cell = Cell::from_window(size.cols, size.rows, size.xpixel, size.ypixel).or(self.cell).unwrap_or(FALLBACK_CELL);
        // The last row is the status line.
        let area_rows = size.rows.saturating_sub(1).max(1);
        let (max_cols, max_rows) = self.output.limits(size.cols);
        let wrap = self.output.wrap();
        let mut out = Vec::new();
        // Pages made for another window, or asked to be made again, go.
        if std::mem::take(&mut self.nav.rerender) || self.size != Some(size) {
            for sent in self.sent.drain(..) {
                kitty::delete(&mut out, sent.id, wrap);
            }
            self.wanted.clear();
            self.failed.clear();
            self.placed.clear();
            self.size = Some(size);
        }
        // Two pages side by side when allowed, fitted to the page, and the window has room: the
        // spread's first page decides.
        let nav = &self.nav;
        let spread_first = nav.pages.get(nav.at - nav.at % 2).and_then(|p| self.doc.sizes.get(*p));
        let pair = nav.two_up
            && nav.fit == Fit::Page
            && nav.pages.len() > 1
            && spread_first.is_some_and(|(w, h)| fit::two_up_fits(*w, *h, max_cols, area_rows, cell, max_rows));
        self.nav.per_screen = if pair { 2 } else { 1 };
        let cols = if pair { fit::two_up_cols(max_cols) } else { max_cols };
        let fit = self.nav.fit;
        let view = |page| View { page, fit, cols, size };
        let on_screen = self.nav.on_screen().map(|page| page.map(view));
        self.on_screen = on_screen;

        // What to have ready, most urgent first: the pages on screen, then the next screens, then
        // the one before, as far as they stay in the terminal together (`budget`). Renders already
        // under way for any of them carry on; the rest stop.
        let mut wanted = Vec::new();
        let mut near = 0u64;
        for v in on_screen.into_iter().flatten().chain(self.nav.around(AHEAD, BEHIND).into_iter().map(view)) {
            if self.failed.iter().any(|(f, _)| *f == v) || wanted.iter().any(|w: &Wanted| w.view == v) {
                continue;
            }
            if let Some(s) = self.sent.iter().find(|s| s.view == v) {
                near = near.saturating_add(s.bytes());
                continue;
            }
            let next = match self.wanted.iter().position(|w| w.view == v) {
                Some(at) => Ok(self.wanted.swap_remove(at)),
                None => self.doc.layout(v.page, v.fit, v.cols, area_rows, cell, max_rows).map(|layout| {
                    self.tag = self.tag.wrapping_add(1);
                    Wanted { view: v, layout, tag: self.tag }
                }),
            };
            match next {
                Ok(w) => {
                    let size = self.doc.sizes.get(v.page).copied().unwrap_or_default();
                    near = near.saturating_add(pixel_bytes(size, w.layout.scale));
                    if near > self.budget && !on_screen.contains(&Some(v)) {
                        break;
                    }
                    wanted.push(w);
                }
                Err(e) => self.fail(v, e),
            }
        }
        self.wanted = wanted;
        self.requeue();

        if self.nav.message.is_none() {
            self.nav.message = note(&on_screen, &self.sent, &self.failed);
        }
        // Scrolling follows the first page on screen; two side by side always fit.
        let first_rows = on_screen[0].and_then(|v| self.rows(&v)).unwrap_or(area_rows);
        self.nav.shown(self.nav.per_screen, first_rows.min(area_rows), if pair { first_rows.min(area_rows) } else { first_rows });

        // Synchronized output, where the terminal has it, shows the redraw all at once.
        out.extend_from_slice(b"\x1b[?2026h");
        let ready = on_screen.iter().flatten().all(|v| self.sent.iter().any(|s| s.view == *v) || self.failed.iter().any(|(f, _)| f == v));
        if ready || self.placed.is_empty() {
            self.place(&mut out, size, area_rows, cols, pair);
        }
        cursor(&mut out, size.rows.saturating_sub(1), 0);
        out.extend_from_slice(b"\x1b[2K\x1b[7m");
        out.extend_from_slice(self.nav.status(self.name, size.cols, !ready).as_bytes());
        out.extend_from_slice(b"\x1b[0m\x1b[?2026l");
        self.wrote(0);
        self.tty.write_all(&out)
    }

    /// Note a write, with `images` transmissions in it: inside tmux, ask a moment later whether
    /// it reached the terminal.
    fn wrote(&mut self, images: u64) {
        self.transmitted = self.transmitted.saturating_add(images);
        if self.watch.is_some() {
            self.tmux_check = Some(Instant::now() + TMUX_AFTER_WRITE);
        }
    }

    /// Inside tmux: ask tmux what became of what was written, and act on it. Says whether the
    /// screen needs a redraw.
    ///
    /// - The pane is hidden: tmux throws passthrough away, so nothing is sent until it shows, and
    ///   files waiting for the terminal are given up on (without giving files up).
    /// - A terminal showing it may lack images (it is new, attached again, or back after images
    ///   went out without it): everything is sent again.
    /// - tmux threw output away (`#{client_discarded}` rose): images went with it (tmux redraws
    ///   its text, but not what went through passthrough), so everything is sent again, up to
    ///   [`MAX_RESENDS`] times.
    /// - More than one terminal shows it: files are over, as the first terminal to read one
    ///   deletes it.
    fn ask_tmux(&mut self) -> bool {
        self.tmux_check = Some(Instant::now() + TMUX_EVERY);
        let (Some(watch), Some(report)) = (self.watch.as_mut(), tmux::report()) else { return false };
        let news = watch.update(&report, self.transmitted);
        if news.showing > 1 {
            self.files = None;
        }
        let was_hidden = std::mem::replace(&mut self.hidden, news.showing == 0);
        if self.hidden {
            if !was_hidden && let Some(files) = self.files.as_mut() {
                files.forget();
            }
            return false;
        }
        if news.discarded {
            if self.resends >= MAX_RESENDS {
                self.nav.message = Some("tmux is dropping images: the terminal can't keep up; r sends them again".into());
                return true;
            }
            self.resends = self.resends.saturating_add(1);
        }
        if news.discarded || news.missed {
            self.nav.rerender = true;
            return true;
        }
        false
    }

    /// The terminal says it no longer has image `id`, which it was asked to place: it dropped it
    /// to stay under its quota. Forget it, so it is sent again, and keep less in the terminal from
    /// now on, up to [`MAX_RESENDS`] times (`r` still sends). Says whether the screen needs a
    /// redraw: past that, only to say so once, since a redraw places the image again and draws
    /// the same answer.
    fn missing(&mut self, id: u32) -> bool {
        const DROPPING: &str = "the terminal is dropping images; r sends them again";
        if !self.sent.iter().any(|s| s.id == id) {
            return false;
        }
        if self.resends >= MAX_RESENDS {
            let said = self.nav.message.as_deref() == Some(DROPPING);
            self.nav.message = Some(DROPPING.into());
            return !said;
        }
        self.resends = self.resends.saturating_add(1);
        self.budget = (self.budget / 2).max(MIN_BUDGET);
        self.sent.retain(|s| s.id != id);
        self.placed.retain(|(placed, _)| *placed != id);
        true
    }

    /// Before the screen is put back: wait a moment for the answers to the last placements, so
    /// none of them reaches the command line.
    fn await_answers(&self) {
        let Some(at) = self.placed_at else { return };
        let deadline = at + ANSWER_WAIT;
        while let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) {
            if self.tty.read_timeout(left).is_err() {
                break;
            }
        }
    }

    /// Put the pages on screen in place of the ones before: centred, one page or two with a gap
    /// (a last page alone keeps the left place). Each place on screen has a placement of its own,
    /// so a page given twice shows twice.
    fn place(&mut self, out: &mut Vec<u8>, size: WindowSize, area_rows: u16, cols: u16, pair: bool) {
        let wrap = self.output.wrap();
        // Erasing text never erases images (only clearing the whole screen does, by the
        // protocol): placements are what show and move them.
        for row in 1..=size.rows {
            out.extend_from_slice(format!("\x1b[{row};1H\x1b[2K").as_bytes());
        }
        let sent = |v: Option<View>| v.and_then(|v| self.sent.iter().position(|s| s.view == v));
        let at = [sent(self.on_screen[0]), sent(self.on_screen[1])];
        let width = |i: usize| at.get(i).copied().flatten().and_then(|p| self.sent.get(p)).map_or(cols, |s| s.cols);
        let widths = [width(0), if pair { width(1) } else { 0 }];
        let total = if pair { widths[0].saturating_add(fit::TWO_UP_GAP).saturating_add(widths[1]) } else { widths[0] };
        let mut left = size.cols.saturating_sub(total) / 2;
        let scroll = if pair { 0 } else { self.nav.scroll };
        let mut placed = Vec::new();
        for ((position, width), placement) in at.into_iter().zip(widths).zip(1u32..) {
            if let Some(s) = position.and_then(|p| self.sent.get(p)) {
                let visible = s.rows.min(area_rows);
                let top = area_rows.saturating_sub(visible) / 2;
                match self.output {
                    Output::Direct => {
                        cursor(out, top, left);
                        let src = (visible < s.rows).then(|| source_rows(s, scroll, visible));
                        kitty::place(out, s.id, placement, s.cols, visible, src, wrap);
                        self.placed_at = Some(Instant::now());
                    }
                    Output::Tmux(colour) => {
                        for row in 0..visible {
                            cursor(out, top.saturating_add(row), left);
                            // Within the placeholders' table: the layout keeps it so.
                            let _ = kitty::placeholder_row(out, s.id, colour, scroll.saturating_add(row), s.cols);
                        }
                    }
                }
                placed.push((s.id, placement));
            }
            left = left.saturating_add(width).saturating_add(fit::TWO_UP_GAP);
        }
        // The pages before leave the screen; their data stays in the terminal for later. Inside
        // tmux, erasing their placeholder cells already did it.
        if self.output == Output::Direct {
            for (id, placement) in self.placed.iter().filter(|p| !placed.contains(p)) {
                kitty::hide(out, *id, *placement, wrap);
            }
        }
        // The pages on screen are the most recently used: the last to be dropped.
        for (id, _) in &placed {
            if let Some(p) = self.sent.iter().position(|s| s.id == *id) {
                let s = self.sent.remove(p);
                self.sent.push(s);
            }
        }
        self.placed = placed;
    }

    /// The image rows `view` covers, once its layout is known.
    fn rows(&self, view: &View) -> Option<u16> {
        let sent = self.sent.iter().find(|s| s.view == *view).map(|s| s.rows);
        sent.or_else(|| self.wanted.iter().find(|w| w.view == *view).map(|w| w.layout.rows))
    }

    /// Take finished renders and send them to the terminal. Says whether one of the pages on
    /// screen arrived (the screen needs a redraw).
    fn receive(&mut self) -> Result<bool, String> {
        let wrap = self.output.wrap();
        let (mut any, mut on_screen) = (false, false);
        while let Some(page) = self.pool.try_recv() {
            // Otherwise a late result for a page no longer wanted.
            let Some(at) = self.wanted.iter().position(|w| w.tag == page.request.tag) else { continue };
            let wanted = self.wanted.remove(at);
            any = true;
            on_screen |= self.on_screen.contains(&Some(wanted.view));
            let id = self.new_id();
            let (width, height) = (page.width, page.height);
            match checked(&page).and_then(|warning| Ok((warning, self.payload(id, page)?))) {
                Ok((warning, Some(payload))) => {
                    let mut out = Vec::new();
                    let bytes = u64::from(width).saturating_mul(u64::from(height)).saturating_mul(4);
                    for gone in make_room(&mut self.sent, bytes, self.budget, &self.on_screen) {
                        kitty::delete(&mut out, gone, wrap);
                        self.placed.retain(|(placed, _)| *placed != gone);
                    }
                    let (cols, rows) = (wanted.layout.cols, wanted.layout.rows);
                    match &payload {
                        Payload::Png(png) => kitty::transmit(&mut out, id, png, wrap),
                        Payload::File(path) => kitty::transmit_file(&mut out, id, path, width, height, wrap),
                    }
                    if let Output::Tmux(_) = self.output {
                        kitty::place_virtual(&mut out, id, cols, rows, wrap);
                    }
                    self.wrote(1);
                    self.tty.write_all(&out)?;
                    self.sent.push(Sent { view: wanted.view, id, cols, rows, width, height, warning });
                }
                // The files' thread kept the page: the next draw asks for it again.
                Ok((_, None)) => on_screen = true,
                Err(e) => self.fail(wanted.view, e),
            }
            // A page the reader waits for goes on screen before any more work ahead is encoded;
            // the other finished renders wait in the pool for the next round.
            if on_screen {
                break;
            }
        }
        if any {
            self.requeue();
        }
        Ok(on_screen)
    }

    /// `page`'s pixels ready to send as image `id`: in a temporary file when the terminal takes
    /// them, otherwise as a PNG. A file that can't be written ends the files, and says why.
    /// `None` when the files' thread stopped answering with the page.
    fn payload(&mut self, id: u32, page: RenderedPage) -> Result<Option<Payload>, String> {
        let now = Instant::now();
        let mut page = page;
        if let Some(files) = self.files.as_mut()
            && files.ready(now)
        {
            match files.write(id, page, now) {
                Written::File(path) => return Ok(Some(Payload::File(path))),
                Written::Failed(e, back) => {
                    self.nav.message = Some(format!("{e}; sending pages in the terminal stream instead"));
                    page = back;
                }
                Written::Stalled => return Ok(None),
            }
        }
        png(&page, self.effort).map(|png| Some(Payload::Png(png)))
    }

    /// Forget the pages whose files the terminal never took, so they are sent again when wanted,
    /// and end the files if their thread stopped answering. Says whether there were any (the
    /// screen needs a redraw).
    fn lost_files(&mut self) -> Result<bool, String> {
        let Some(files) = self.files.as_mut() else { return Ok(false) };
        let lost = files.check(Instant::now());
        if files.stalled() {
            self.nav.message = Some("the temporary directory stopped answering; sending pages in the terminal stream instead".into());
            self.stalled_files = self.files.take();
        }
        if lost.is_empty() {
            return Ok(false);
        }
        // The terminal may have taken one after all, late: none is left behind in it.
        let mut out = Vec::new();
        for id in &lost {
            kitty::delete(&mut out, *id, self.output.wrap());
        }
        self.wrote(0);
        self.tty.write_all(&out)?;
        self.sent.retain(|s| !lost.contains(&s.id));
        self.placed.retain(|(id, _)| !lost.contains(id));
        Ok(true)
    }

    fn fail(&mut self, view: View, why: String) {
        if self.failed.len() >= MAX_FAILED {
            self.failed.remove(0);
        }
        self.failed.push((view, why));
    }

    /// The next image id: one never used in this run until the ids wrap (after 65,280 of them in
    /// the palette, far more than the pages kept).
    fn new_id(&mut self) -> u32 {
        let id = self.output.ids().id(self.first_id.wrapping_add(self.next_id));
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Tell the pool which renders are still wanted, most urgent first: it keeps those and
    /// cancels the rest.
    fn requeue(&self) {
        self.pool.set_queue(self.wanted.iter().map(|w| request(w.view.page, &w.layout, w.tag)).collect());
    }
}

/// Room in the terminal for a page of `incoming` bytes: the least recently shown pages not
/// `on_screen` go while keeping them would make more than [`MAX_SENT`] pages, or more than
/// `budget` bytes. The ids of the ones that go.
fn make_room(sent: &mut Vec<Sent>, incoming: u64, budget: u64, on_screen: &[Option<View>; 2]) -> Vec<u32> {
    let mut gone = Vec::new();
    loop {
        let bytes = sent.iter().map(Sent::bytes).fold(incoming, u64::saturating_add);
        if sent.len() < MAX_SENT && bytes <= budget {
            break;
        }
        let Some(at) = sent.iter().position(|s| !on_screen.contains(&Some(s.view))) else { break };
        gone.push(sent.remove(at).id);
    }
    gone
}

/// The bytes of pixels a page `size` points large renders to at `scale` pixels per point.
fn pixel_bytes((w, h): (f32, f32), scale: f32) -> u64 {
    // Finite and clamped to the largest side a layout allows, so the cast is exact.
    let side = |points: f32| {
        let pixels = (points * scale).ceil();
        if pixels.is_finite() { pixels.clamp(0.0, fit::MAX_PIXELS) as u64 } else { 0 }
    };
    side(w).saturating_mul(side(h)).saturating_mul(4)
}

/// What the status line says about the pages on screen, the first page's first: why one failed,
/// or what was skipped on one.
fn note(on_screen: &[Option<View>; 2], sent: &[Sent], failed: &[(View, String)]) -> Option<String> {
    on_screen.iter().flatten().find_map(|v| {
        let failure = failed.iter().find(|(f, _)| f == v).map(|(_, e)| e.clone());
        failure.or_else(|| sent.iter().find(|s| s.view == *v).and_then(|s| s.warning.clone()))
    })
}

/// Move the cursor to 0-based `row` and `col`.
fn cursor(out: &mut Vec<u8>, row: u16, col: u16) {
    out.extend_from_slice(format!("\x1b[{};{}H", u32::from(row) + 1, u32::from(col) + 1).as_bytes());
}

/// The pixels of `shown` that `visible` cell rows starting at row `scroll` cover.
fn source_rows(shown: &Sent, scroll: u16, visible: u16) -> Rect {
    let per_row = f64::from(shown.height) / f64::from(shown.rows.max(1));
    let height = f64::from(shown.height);
    // Finite, and clamped to the image's height, so the casts are exact.
    let y = (f64::from(scroll) * per_row).round().clamp(0.0, height);
    let h = (f64::from(visible) * per_row).round().clamp(0.0, height - y);
    Rect { x: 0, y: y as u32, w: shown.width, h: h as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader at `at` in a `count`-page document, after a draw that showed `visible` of a
    /// page's `rows` rows, one page to the screen.
    fn reader(at: usize, count: usize, visible: u16, rows: u16) -> Nav {
        let mut nav = Nav::new((0..count).collect(), count, Fit::Page);
        nav.go(at);
        nav.shown(1, visible, rows);
        nav
    }

    /// The page on screen (the first, when two are).
    fn page(nav: &Nav) -> usize {
        nav.on_screen()[0].unwrap()
    }

    fn press(nav: &mut Nav, keys: &[Key]) -> Step {
        keys.iter().map(|k| nav.key(*k)).find(|s| *s == Step::Quit).unwrap_or(Step::Continue)
    }

    #[test]
    fn space_turns_pages_that_fit_and_stops_at_the_last() {
        let mut nav = reader(0, 3, 40, 40);
        press(&mut nav, &[Key::Char(' ')]);
        assert_eq!((page(&nav), nav.scroll), (1, 0));
        press(&mut nav, &[Key::PageDown, Key::Char(' '), Key::Char(' ')]);
        assert_eq!(page(&nav), 2);
        press(&mut nav, &[Key::Char('b')]);
        assert_eq!(page(&nav), 1);
    }

    #[test]
    fn a_tall_page_scrolls_before_the_next_one() {
        // 30 of 100 rows on screen: space scrolls 30, 30, then the last 10, then turns the page.
        let mut nav = reader(0, 2, 30, 100);
        for expected in [30, 60, 70] {
            press(&mut nav, &[Key::Char(' ')]);
            assert_eq!((page(&nav), nav.scroll), (0, expected));
        }
        press(&mut nav, &[Key::Char(' ')]);
        assert_eq!((page(&nav), nav.scroll), (1, 0));
        // Back from the top of page 2 lands on the bottom of page 1, once the draw clamps it.
        press(&mut nav, &[Key::Char('b')]);
        assert_eq!(page(&nav), 0);
        nav.shown(1, 30, 100);
        assert_eq!(nav.scroll, 70);
        press(&mut nav, &[Key::Char('k')]);
        assert_eq!(nav.scroll, 70 - STEP);
        press(&mut nav, &[Key::Char('j')]);
        assert_eq!((page(&nav), nav.scroll), (0, 70));
        // At the bottom, a row more is the next page.
        press(&mut nav, &[Key::Down]);
        assert_eq!((page(&nav), nav.scroll), (1, 0));
        nav.shown(1, 30, 100);
        press(&mut nav, &[Key::Enter]);
        assert_eq!((page(&nav), nav.scroll), (1, STEP));
    }

    #[test]
    fn page_keys_jump() {
        let mut nav = reader(4, 10, 40, 40);
        press(&mut nav, &[Key::Char('n')]);
        assert_eq!(page(&nav), 5);
        press(&mut nav, &[Key::Left, Key::Char('p')]);
        assert_eq!(page(&nav), 3);
        press(&mut nav, &[Key::Char('G')]);
        assert_eq!(page(&nav), 9);
        press(&mut nav, &[Key::Right]);
        assert_eq!(page(&nav), 9, "past the last page stays on it");
        press(&mut nav, &[Key::Home]);
        assert_eq!(page(&nav), 0);
        press(&mut nav, &[Key::Left]);
        assert_eq!(page(&nav), 0);
    }

    #[test]
    fn two_pages_side_by_side_turn_like_a_book() {
        let mut nav = reader(0, 5, 40, 40);
        nav.shown(2, 40, 40);
        assert_eq!(nav.on_screen(), [Some(0), Some(1)]);
        for (keys, expected) in [
            (&[Key::Char(' ')][..], [Some(2), Some(3)]),
            (&[Key::Char('n')], [Some(4), None]),
            (&[Key::Char('n')], [Some(4), None]),
            (&[Key::Char('b')], [Some(2), Some(3)]),
            (&[Key::Char('p')], [Some(0), Some(1)]),
            (&[Key::Char('p')], [Some(0), Some(1)]),
            (&[Key::Char('G')], [Some(4), None]),
        ] {
            press(&mut nav, keys);
            assert_eq!(nav.on_screen(), expected, "{keys:?}");
        }
        // A typed page shows the spread it is on: page 4 is the right page of 3–4.
        press(&mut nav, &[Key::Char('4'), Key::Enter]);
        assert_eq!(nav.on_screen(), [Some(2), Some(3)]);
        // One to a screen again from where the reader is, at once: a key before the next draw
        // steps by one page.
        press(&mut nav, &[Key::Char('d')]);
        assert!(!nav.two_up);
        assert_eq!(nav.on_screen(), [Some(3), None]);
        press(&mut nav, &[Key::Char('p')]);
        assert_eq!(nav.on_screen(), [Some(2), None]);
        // Fitting the width shows one page too.
        press(&mut nav, &[Key::Char('d')]);
        nav.shown(2, 40, 40);
        press(&mut nav, &[Key::Char('f'), Key::Char('n')]);
        assert_eq!(nav.on_screen(), [Some(3), None]);
    }

    #[test]
    fn the_pages_worked_on_ahead_are_the_next_screens_then_the_one_before() {
        let mut nav = reader(4, 10, 40, 40);
        assert_eq!(nav.around(2, 1), [5, 6, 3]);
        // Two to a screen, 4–5 on it.
        nav.shown(2, 40, 40);
        assert_eq!(nav.around(2, 1), [6, 7, 8, 9, 2, 3]);
        // At the ends, only what exists.
        assert_eq!(reader(9, 10, 40, 40).around(2, 1), [8]);
        assert!(reader(0, 1, 40, 40).around(2, 1).is_empty());
        // However many screens are asked for, the ends stop it.
        assert!(Nav::new(Vec::new(), 0, Fit::Page).around(usize::MAX, usize::MAX).is_empty());
        assert_eq!(reader(1, 3, 40, 40).around(usize::MAX, usize::MAX), [2, 0]);
    }

    #[test]
    fn pages_given_are_the_only_ones_paged_through() {
        let mut nav = Nav::new(vec![6, 2, 9], 12, Fit::Page);
        nav.shown(1, 40, 40);
        assert_eq!(page(&nav), 6);
        press(&mut nav, &[Key::Char(' ')]);
        assert_eq!(page(&nav), 2);
        press(&mut nav, &[Key::Char('G')]);
        assert_eq!(page(&nav), 9);
        press(&mut nav, &[Key::Char('7'), Key::Enter]);
        assert_eq!(page(&nav), 6);
        press(&mut nav, &[Key::Char('5'), Key::Enter]);
        assert_eq!(page(&nav), 6);
        assert_eq!(nav.message.as_deref(), Some("page 5 isn't one of the pages given"));
        assert!(nav.status("a.pdf", 120, false).contains(" page 7/12 · fit page · 1 of 3 "));
    }

    #[test]
    fn a_typed_number_goes_to_that_page() {
        let mut nav = reader(0, 12, 40, 40);
        press(&mut nav, &[Key::Char('1'), Key::Char('2'), Key::Enter]);
        assert_eq!((page(&nav), nav.typed.as_str()), (11, ""));
        press(&mut nav, &[Key::Char('3'), Key::Char('g')]);
        assert_eq!(page(&nav), 2);
        press(&mut nav, &[Key::Char('9'), Key::Char('9'), Key::Enter]);
        assert_eq!(page(&nav), 2);
        assert_eq!(nav.message.as_deref(), Some("no page 99 (1–12)"));
        press(&mut nav, &[Key::Char('0'), Key::Enter]);
        assert_eq!(page(&nav), 2);
        // Backspace and Esc edit and cancel; Esc with nothing typed quits.
        press(&mut nav, &[Key::Char('5'), Key::Char('1'), Key::Backspace, Key::Enter]);
        assert_eq!(page(&nav), 4);
        assert_eq!(press(&mut nav, &[Key::Char('7'), Key::Escape]), Step::Continue);
        assert_eq!(nav.typed, "");
        assert_eq!(press(&mut nav, &[Key::Escape]), Step::Quit);
        // Long numbers stop growing and simply name no page.
        let mut nav = reader(0, 12, 40, 40);
        press(&mut nav, &[Key::Char('9'); 30]);
        assert_eq!(nav.typed.len(), MAX_TYPED);
    }

    #[test]
    fn q_and_ctrl_c_quit_and_f_switches_the_fit() {
        let mut nav = reader(0, 2, 30, 100);
        press(&mut nav, &[Key::Char(' '), Key::Char('f')]);
        assert_eq!((nav.fit, nav.scroll), (Fit::Width, 0));
        press(&mut nav, &[Key::Char('f')]);
        assert_eq!(nav.fit, Fit::Page);
        press(&mut nav, &[Key::Char('r')]);
        assert!(nav.rerender);
        assert_eq!(press(&mut nav, &[Key::Char('q')]), Step::Quit);
        assert_eq!(press(&mut nav, &[Key::Interrupt]), Step::Quit);
        assert_eq!(press(&mut nav, &[Key::Char('x'), Key::PageUp, Key::End]), Step::Continue);
    }

    #[test]
    fn an_empty_list_shows_no_page_and_never_panics() {
        let mut nav = Nav::new(Vec::new(), 0, Fit::Page);
        nav.shown(2, 1, 1);
        assert_eq!(nav.on_screen(), [None, None]);
        press(&mut nav, &[Key::Char(' '), Key::Char('b'), Key::Char('n'), Key::Char('p'), Key::Char('G'), Key::Char('1'), Key::Enter]);
        assert!(nav.status("a.pdf", 80, false).contains("no page"));
    }

    #[test]
    fn the_status_line_fills_the_width_exactly() {
        let nav = reader(2, 12, 40, 40);
        for cols in [0, 1, 10, 40, 80, 200] {
            let line = nav.status("report.pdf", cols, false);
            assert_eq!(line.chars().count(), usize::from(cols), "{line:?}");
        }
        let wide = nav.status("report.pdf", 120, false);
        assert!(wide.starts_with(" report.pdf · page 3/12 · fit page ") && wide.ends_with("q quit "), "{wide:?}");
        let mut two = reader(2, 12, 40, 40);
        two.shown(2, 40, 40);
        assert!(two.status("report.pdf", 120, false).starts_with(" report.pdf · pages 3–4/12 · fit page "));
        let mut typing = nav;
        typing.key(Key::Char('7'));
        assert!(typing.status("a.pdf", 80, false).contains("go to page 7"));
    }

    #[test]
    fn a_narrow_status_line_keeps_its_two_sides_apart() {
        let nav = reader(4, 13, 40, 40);
        let left = " pdfcraft-showcase.pdf · page 5/13 · fit page ";
        // Half a wide window: the shortest reminder of the keys, two spaces from the page.
        let half = nav.status("pdfcraft-showcase.pdf", 70, false);
        assert!(half.starts_with(left) && half.ends_with("  n p · q quit "), "{half:?}");
        for cols in 0..=120 {
            let line = nav.status("pdfcraft-showcase.pdf", cols, false);
            assert_eq!(line.chars().count(), usize::from(cols), "{line:?}");
            // Whatever follows the page is a whole reminder of the keys, two spaces or more after
            // it (`left` ends with one), or nothing.
            if let Some(rest) = line.strip_prefix(left) {
                let keys = rest.trim_start();
                assert!(keys.is_empty() || (rest.starts_with(' ') && KEYS.iter().any(|k| keys == format!("{k} "))), "{line:?}");
            }
        }
        // The file name gives way to the page, and to a message, which stays whole.
        let long = "a file name long enough to fill most of the window.pdf";
        assert_eq!(nav.status(long, 50, false), " a file name long enough… · page 5/13 · fit page  ");
        let mut told = nav;
        told.message = Some("no page 99 (1–13)".into());
        assert_eq!(told.status(long, 60, false), " a file name lon… · page 5/13 · fit page  no page 99 (1–13) ");
    }

    #[test]
    fn the_status_line_never_carries_an_escape_sequence() {
        let mut nav = reader(0, 3, 40, 40);
        // A file name and an error message that would write to the clipboard (OSC 52).
        nav.message = Some("page 1: renderer crashed: \x1b]52;c;cHduZWQ=\x07".into());
        let line = nav.status("evil\x1b]52;c;cHduZWQ=\x07.pdf", 200, false);
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert_eq!(line.chars().count(), 200);
        nav.message = None;
        assert!(nav.status("a.pdf", 80, true).contains("rendering…"));
    }

    /// Page `page` sent as image `id`, `mb` megabytes of pixels, with `warning`.
    fn sent(page: usize, id: u32, mb: u32, warning: Option<&str>) -> Sent {
        let view = View { page, fit: Fit::Page, cols: 50, size: FALLBACK_SIZE };
        Sent { view, id, cols: 50, rows: 40, width: 1024, height: mb * 256, warning: warning.map(String::from) }
    }

    #[test]
    fn pages_kept_in_the_terminal_stay_under_the_count_and_the_budget() {
        let mut kept: Vec<Sent> = (0..4).map(|p| sent(p, p as u32 + 1, 10, None)).collect();
        let screen = [Some(kept[0].view), None];
        // 40 MB kept, 10 more coming: under 64, nothing goes.
        assert!(make_room(&mut kept, 10 << 20, 64 << 20, &screen).is_empty());
        // Under 32: the least recently shown go first, never the page on screen.
        assert_eq!(make_room(&mut kept, 10 << 20, 32 << 20, &screen), [2, 3]);
        assert_eq!(kept.iter().map(|s| s.id).collect::<Vec<_>>(), [1, 4]);
        // A page bigger than the whole budget: all but the screen go, and it goes in anyway.
        assert_eq!(make_room(&mut kept, 100 << 20, 32 << 20, &screen), [4]);
        assert_eq!(kept.len(), 1);
        // And never more than MAX_SENT pages, however small.
        let mut many: Vec<Sent> = (0..MAX_SENT).map(|p| sent(p, p as u32, 1, None)).collect();
        assert_eq!(make_room(&mut many, 1, u64::MAX, &[None, None]), [0]);
        // Sizes that would overflow don't.
        let mut huge = vec![Sent { width: u32::MAX, height: u32::MAX, ..sent(0, 1, 1, None) }];
        assert_eq!(make_room(&mut huge, u64::MAX, BUDGET, &[None, None]), [1]);
    }

    #[test]
    fn a_page_takes_four_bytes_a_pixel() {
        // US Letter at 2 px per point: 1224 × 1584 pixels.
        assert_eq!(pixel_bytes((612.0, 792.0), 2.0), 1224 * 1584 * 4);
        assert_eq!(pixel_bytes((0.5, 0.5), 1.0), 4, "a part of a pixel is a pixel");
        assert_eq!(pixel_bytes((f32::MAX, 1.0), 2.0), 0, "an endless side counts for nothing");
        assert_eq!(pixel_bytes((1e6, 1e6), 1.0), 8192 * 8192 * 4);
        assert_eq!(pixel_bytes((f32::NAN, -5.0), 1.0), 0);
    }

    #[test]
    fn the_status_line_names_what_is_wrong_with_a_page_on_screen_whenever_it_is() {
        let kept = [sent(0, 1, 1, None), sent(1, 2, 1, Some("page 2: content past the page's safety budget was skipped"))];
        let view = |page| Some(View { page, fit: Fit::Page, cols: 50, size: FALLBACK_SIZE });
        // Page 2 was rendered ahead with something skipped: said when it shows, not before.
        assert_eq!(note(&[view(0), None], &kept, &[]), None);
        assert_eq!(note(&[view(1), None], &kept, &[]).as_deref(), Some("page 2: content past the page's safety budget was skipped"));
        assert!(note(&[view(0), view(1)], &kept, &[]).is_some());
        // A failure first.
        let failed = [(view(0).unwrap(), "page 1: broken".to_string())];
        assert_eq!(note(&[view(0), view(1)], &kept, &failed).as_deref(), Some("page 1: broken"));
    }

    #[test]
    fn source_rows_follow_the_scroll() {
        let view = View { page: 0, fit: Fit::Width, cols: 50, size: FALLBACK_SIZE };
        let shown = Sent { view, id: 1, cols: 50, rows: 100, width: 1000, height: 2000, warning: None };
        assert_eq!(source_rows(&shown, 0, 30), Rect { x: 0, y: 0, w: 1000, h: 600 });
        assert_eq!(source_rows(&shown, 70, 30), Rect { x: 0, y: 1400, w: 1000, h: 600 });
        // Never past the bottom, whatever it is asked.
        let past = source_rows(&shown, u16::MAX, 30);
        assert_eq!((past.y, past.h), (2000, 0));
        let empty = Sent { rows: 0, height: 0, ..shown };
        assert_eq!(source_rows(&empty, 5, 5), Rect { x: 0, y: 0, w: 1000, h: 0 });
    }
}
