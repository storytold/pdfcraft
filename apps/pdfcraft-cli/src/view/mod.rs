//! `pdfcraft-cli view`: PDF pages drawn straight into the terminal through the kitty graphics
//! protocol (kitty, Ghostty, WezTerm and others). Usage and behaviour: `docs/cli-view.md`.

mod detect;
mod files;
mod fit;
mod keys;
mod kitty;
mod pager;
mod tmux;
mod tty;

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pdfcraft_engine::export::PngEffort;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderPool, RenderRequest, RenderWarning, RenderedPage, RequestKind};

use crate::{CliError, OptionSpec, displayed, flag, page_list, read, validate_options};
use fit::{Area, Cell, Fit, Grid, Layout, Zoom};
use kitty::{IdColor, Wrap};
use tty::{Tty, WindowSize};

const OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--pages", takes_value: true },
    OptionSpec { name: "--zoom", takes_value: true },
    OptionSpec { name: "--protocol", takes_value: true },
    OptionSpec { name: "--password", takes_value: true },
    OptionSpec { name: "--pager", takes_value: false },
    OptionSpec { name: "--no-pager", takes_value: false },
    OptionSpec { name: "--compression", takes_value: true },
];

/// The window assumed when there is no terminal to ask (no controlling terminal, as under CI):
/// the classic 80 × 24, at [`fit::FALLBACK_CELL`].
const FALLBACK_SIZE: WindowSize = WindowSize { cols: 80, rows: 24, xpixel: 0, ypixel: 0 };

/// How long the terminal gets to answer a query (support, cell size). Terminals answer within
/// milliseconds, so this only matters for one that never does.
const QUERY_TIMEOUT: Duration = Duration::from_millis(500);

const NOT_A_TERMINAL: &str = "view: stdout is not a terminal (`pdfcraft-cli render --out page.png` writes an image file; \
                              --protocol kitty sends the escape codes anyway)";
const UNSUPPORTED: &str = "view: this terminal doesn't show images through the kitty graphics protocol (kitty, Ghostty \
                           and WezTerm do; `pdfcraft-cli render --out page.png` writes an image file instead)";

/// While the pager owns the screen, panic messages aren't printed over it (see [`run`]).
static QUIET: AtomicBool = AtomicBool::new(false);

/// Which graphics protocol: found out by asking, or taken on trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Protocol {
    Auto,
    Kitty,
}

impl Protocol {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Self::Auto),
            "kitty" => Ok(Self::Kitty),
            other => Err(format!("view: --protocol {other:?}: use auto or kitty")),
        }
    }
}

/// How pages reach the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Output {
    /// Shown at the cursor.
    Direct,
    /// Through tmux's passthrough, and drawn by placeholder cells whose colour names the image.
    Tmux(IdColor),
}

impl Output {
    /// The most columns and rows a page may cover in a window `cols` wide: placeholders number
    /// at most [`kitty::MAX_CELLS`] of either.
    fn limits(self, cols: u16) -> (u16, u16) {
        match self {
            Self::Direct => (cols, u16::MAX),
            Self::Tmux(_) => (cols.min(kitty::MAX_CELLS), kitty::MAX_CELLS),
        }
    }

    fn wrap(self) -> Wrap {
        match self {
            Self::Direct => Wrap::None,
            Self::Tmux(_) => Wrap::Tmux,
        }
    }

    /// The image ids this output can draw with: any outside tmux, as many as placeholder colours
    /// can name inside it.
    fn ids(self) -> IdColor {
        match self {
            Self::Direct => IdColor::Rgb,
            Self::Tmux(colour) => colour,
        }
    }
}

/// Whether pages are paged through or printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Paging {
    /// The pager when there is more than a page to show, on a terminal it can read keys from;
    /// otherwise printed, as `less -F` does when the text fits.
    Auto,
    /// `--pager`: always the pager, even for one page.
    Always,
    /// `--no-pager`: always printed.
    Never,
}

impl Paging {
    fn from_flags(pager: bool, no_pager: bool) -> Result<Self, String> {
        match (pager, no_pager) {
            (true, true) => Err("view: give --pager or --no-pager, not both".into()),
            (true, false) => Ok(Self::Always),
            (false, true) => Ok(Self::Never),
            (false, false) => Ok(Self::Auto),
        }
    }

    /// Whether `pages` pages are paged through, `interactive` being whether stdout is a terminal
    /// and keys can be read from one.
    fn pages(self, pages: usize, interactive: bool) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => interactive && pages > 1,
        }
    }
}

/// How hard pages are compressed on their way to the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Compression {
    /// `small` over SSH. On this machine, `fast`, and the pager hands pages over through
    /// temporary files when the terminal takes them (`files.rs`).
    Auto,
    /// Least CPU: for a terminal on the same machine, where encoding is most of a page's cost.
    Fast,
    /// Fewest bytes: for a terminal across a network, where the bytes are.
    Small,
}

impl Compression {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Self::Auto),
            "fast" => Ok(Self::Fast),
            "small" => Ok(Self::Small),
            other => Err(format!("view: --compression {other:?}: use auto, fast or small")),
        }
    }

    /// The PNG effort, and whether the pager may try temporary files, asking `over_ssh` only
    /// when it matters. A compression asked for by name means PNGs in the stream.
    fn choose(self, over_ssh: impl FnOnce() -> bool) -> (PngEffort, bool) {
        match self {
            Self::Fast => (PngEffort::Fast, false),
            Self::Small => (PngEffort::Small, false),
            Self::Auto if over_ssh() => (PngEffort::Small, false),
            Self::Auto => (PngEffort::Fast, true),
        }
    }
}

/// Whether the terminal is at the other end of an SSH connection, so the bytes `view` sends
/// cross a network. Inside tmux, the session knows how its client attached (`tmux_says`); the
/// pane's own environment may date from another attach.
fn over_ssh(env_ssh: bool, tmux_says: Option<bool>) -> bool {
    tmux_says.unwrap_or(env_ssh)
}

/// Whether this process's environment comes from an SSH login.
fn env_ssh() -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"].iter().any(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
}

/// The terminal `view` draws on.
struct Terminal {
    output: Output,
    /// The cell size tmux or the terminal reports, for a tty that reports no pixel size.
    cell: Option<Cell>,
    /// How hard pages are compressed for it.
    effort: PngEffort,
    /// Whether the pager may hand pages over through temporary files: the terminal may be on
    /// this machine.
    files: bool,
}

pub(super) fn run(args: &[String]) -> Result<(), CliError> {
    // A document can make the renderer panic. The renderer catches the panic and reports the page
    // as failed, but the default hook first prints the panic's message, which the document may
    // have shaped, raw to the terminal. This hook prints it made safe, and not over the pager.
    std::panic::set_hook(Box::new(|info| {
        if !QUIET.load(Ordering::Relaxed) {
            note(&format!("pdfcraft-cli: {info}"));
        }
    }));
    view(args).map_err(|e| match e {
        CliError::Message(message) => CliError::Message(printable(&message)),
        other => other,
    })
}

fn view(args: &[String]) -> Result<(), CliError> {
    validate_options("view", args, OPTIONS)?;
    let path = file_arg(args)?;
    let zoom = flag(args, "--zoom").map_or(Ok(Zoom::Auto), Zoom::parse)?;
    let protocol = Protocol::parse(flag(args, "--protocol").unwrap_or("auto"))?;
    let compression = Compression::parse(flag(args, "--compression").unwrap_or("auto"))?;
    let doc = Doc::open(path, flag(args, "--password"))?;
    let pages = match flag(args, "--pages") {
        Some(list) => page_list(list).map_err(|e| format!("view: --pages: {e}"))?,
        None => (0..doc.count).collect(),
    };
    if let Some(page) = pages.iter().find(|p| **p >= doc.count) {
        let plural = if doc.count == 1 { "" } else { "s" };
        return Err(format!("view: no page {} ({path} has {} page{plural})", page.saturating_add(1), doc.count).into());
    }
    let has = |name: &str| args.iter().any(|a| a == name);
    let paging = Paging::from_flags(has("--pager"), has("--no-pager"))?;
    let tty = Tty::open().ok();
    let paging = paging.pages(pages.len(), std::io::stdout().is_terminal() && tty.is_some());
    let inside_tmux = tmux::inside();
    // Paging, the terminal stays in raw mode from the first question on (outside tmux, which is
    // asked instead), never back in canonical mode between a question and the pager. On FreeBSD a
    // read begun in canonical mode stays a line read after raw mode returns: the reader thread,
    // its next read begun in between, would hold every key until Enter.
    let _raw = match (&tty, paging && !inside_tmux) {
        (Some(tty), true) => Some(tty.raw().map_err(|e| format!("view: {e}"))?),
        _ => None,
    };
    let terminal = if inside_tmux {
        tmux_terminal(protocol)?
    } else {
        check_terminal(protocol, tty.as_ref())?;
        let cell = tty.as_ref().and_then(ask_cell);
        Terminal { output: Output::Direct, cell, effort: PngEffort::Fast, files: false }
    };
    let (effort, files) = compression.choose(|| over_ssh(env_ssh(), inside_tmux.then(tmux::attached_over_ssh).flatten()));
    let terminal = Terminal { effort, files, ..terminal };
    let first_id = fresh_index();
    if paging {
        let tty = tty.as_ref().ok_or("view: --pager needs a terminal to read keys from")?;
        let name = std::path::Path::new(path).file_name().map_or_else(|| path.into(), |n| n.to_string_lossy());
        return pager::run(tty, &doc, &name, &terminal, pages, zoom, first_id).map_err(|e| format!("view: {e}").into());
    }
    let size = tty.as_ref().and_then(Tty::size).unwrap_or(FALLBACK_SIZE);
    print_pages(&doc, &pages, zoom, size, &terminal, first_id)
}

/// The one file argument: the token that is neither an option nor an option's value.
fn file_arg(args: &[String]) -> Result<&str, String> {
    let mut files = Vec::new();
    let mut i = 0usize;
    while let Some(arg) = args.get(i) {
        match OPTIONS.iter().find(|o| o.name == arg) {
            Some(option) => i = i.saturating_add(if option.takes_value { 2 } else { 1 }),
            None => {
                files.push(arg.as_str());
                i = i.saturating_add(1);
            }
        }
    }
    match files.as_slice() {
        [file] => Ok(file),
        [] => Err("view: missing file".into()),
        _ => Err(format!("view: one file at a time (got {})", files.join(", "))),
    }
}

/// Make sure stdout is a terminal that shows kitty images, unless `--protocol kitty` says to
/// trust it. The environment answers for kitty and Ghostty; any other terminal is asked.
fn check_terminal(protocol: Protocol, tty: Option<&Tty>) -> Result<(), CliError> {
    if protocol == Protocol::Kitty {
        return Ok(());
    }
    if !std::io::stdout().is_terminal() {
        return Err(NOT_A_TERMINAL.into());
    }
    let var = |name| std::env::var(name).ok();
    if detect::env_says_kitty(var("TERM").as_deref(), var("TERM_PROGRAM").as_deref(), std::env::var_os("KITTY_WINDOW_ID").is_some()) {
        return Ok(());
    }
    let tty = tty.ok_or(UNSUPPORTED)?;
    if ask_terminal(tty)? { Ok(()) } else { Err(UNSUPPORTED.into()) }
}

/// Send the support query and read the answer.
fn ask_terminal(tty: &Tty) -> Result<bool, String> {
    let reply = ask(tty, detect::QUERY, |reply| detect::parse_reply(reply).done)?;
    Ok(detect::parse_reply(&reply).graphics)
}

/// The cell size, from the terminal, when its tty reports no pixel size: `docker exec` and
/// `kubectl exec` pass on rows and columns only. `None` when the tty reports pixels, stdout is
/// somewhere else, or the terminal doesn't say; tmux says for its client instead
/// ([`tmux_terminal`]).
fn ask_cell(tty: &Tty) -> Option<Cell> {
    let size = tty.size()?;
    if Cell::from_window(size.cols, size.rows, size.xpixel, size.ypixel).is_some() || !std::io::stdout().is_terminal() {
        return None;
    }
    let reply = ask(tty, detect::CELL_QUERY, |reply| detect::parse_cell_reply(reply).done).ok()?;
    detect::parse_cell_reply(&reply).cell.map(|(w, h)| Cell { w: f32::from(w), h: f32::from(h) })
}

/// Send `query` and read the answer until `done` says it is all there, or for [`QUERY_TIMEOUT`]
/// at most. Raw mode keeps the reply off the screen and hands it over without waiting for Enter.
fn ask(tty: &Tty, query: &[u8], done: impl Fn(&[u8]) -> bool) -> Result<Vec<u8>, String> {
    let _raw = tty.raw()?;
    tty.write_all(query)?;
    let deadline = Instant::now() + QUERY_TIMEOUT;
    let mut reply = Vec::new();
    while reply.len() < detect::MAX_REPLY {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let Some(bytes) = tty.read_timeout(left)? else { break };
        reply.extend_from_slice(&bytes);
        if done(&reply) {
            break;
        }
    }
    Ok(reply)
}

/// Inside tmux: ask tmux which terminal its client runs in (`TERM` names tmux itself), and
/// whether the pane passes escape codes through. When it doesn't, `view` stops, and leaves the
/// setting as it is ([`tmux::passthrough`]).
fn tmux_terminal(protocol: Protocol) -> Result<Terminal, CliError> {
    let to_terminal = std::io::stdout().is_terminal();
    if protocol == Protocol::Auto && !to_terminal {
        return Err(NOT_A_TERMINAL.into());
    }
    let client = tmux::client();
    let colour = tmux_colour(protocol, &client)?;
    match tmux::passthrough() {
        Ok(true) => {}
        // Nothing would show. Written elsewhere, the output may yet be shown where it is on.
        Ok(false) if to_terminal => return Err(format!("view: {}", tmux::passthrough_off()).into()),
        Ok(false) => note(&format!("warning: view: {}", tmux::passthrough_off())),
        Err(e) => note(&format!("warning: view: couldn't check tmux's allow-passthrough ({e}); images may not show")),
    }
    let cell = client.ok().and_then(|c| c.cell).map(|(w, h)| Cell { w: f32::from(w), h: f32::from(h) });
    Ok(Terminal { output: Output::Tmux(colour), cell, effort: PngEffort::Fast, files: false })
}

/// The colour placeholder cells name images with, once the terminal outside tmux is known to
/// speak the protocol (or `--protocol kitty` says to trust it): 24-bit when tmux passes it
/// through unchanged, the 256-colour palette otherwise.
fn tmux_colour(protocol: Protocol, client: &Result<tmux::Client, String>) -> Result<IdColor, String> {
    match (protocol, client) {
        (Protocol::Auto, Err(e)) => {
            Err(format!("view: inside tmux, but tmux didn't say which terminal it runs in ({e}); --protocol kitty tries anyway"))
        }
        (Protocol::Auto, Ok(c)) if !detect::term_name_says_kitty(&c.termname) => {
            let name = if c.termname.is_empty() { "unknown" } else { &c.termname };
            Err(format!(
                "view: tmux runs in a terminal ({name}) not known to show images through the kitty graphics protocol \
                 (kitty and Ghostty are); --protocol kitty tries anyway"
            ))
        }
        (_, Ok(c)) if c.rgb => Ok(IdColor::Rgb),
        _ => Ok(IdColor::Indexed),
    }
}

/// `text` made safe to write to a terminal: no control characters, so none of the ESC, CSI or
/// OSC introducers that start escape sequences (a clipboard write, a title change, a graphics
/// command). Line breaks and tabs become spaces, the others U+FFFD. Everything a document or a
/// file name can shape goes through this on its way to the terminal.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' | '\r' | '\t' => ' ',
            c if c.is_control() => '\u{FFFD}',
            c => c,
        })
        .collect()
}

/// A line on stderr that doesn't stop `view`, made safe for the terminal.
fn note(line: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{}", printable(line));
}

/// Draw `pages` at the cursor, one row of them under another, like `cat`: as many to a row as the
/// pager shows at `zoom` from the first page, each row fitted the way the pager fits the reader's
/// row. A page that fails is reported at the end; the others are still drawn.
fn print_pages(doc: &Doc, pages: &[usize], zoom: Zoom, size: WindowSize, terminal: &Terminal, first_id: u64) -> Result<(), CliError> {
    let mut renderer = doc.renderer();
    if renderer.page_count() == 0 {
        return Err("view: the renderer can't read this document".into());
    }
    let cell = Cell::from_window(size.cols, size.rows, size.xpixel, size.ypixel).or(terminal.cell).unwrap_or(fit::FALLBACK_CELL);
    // One row stays free for the shell prompt after the last page.
    let rows = size.rows.saturating_sub(1).max(1);
    let (cols, max_rows) = terminal.output.limits(size.cols);
    let area = Area { cols, rows, cell, max_rows };
    let size_of = |i: usize| pages.get(i).and_then(|p| doc.sizes.get(*p)).copied();
    // Every row has as many pages as the first: `auto` decides once.
    let first = fit::grid(zoom, pages.len(), 0, size_of, area);
    let zoom = if first.fit == Fit::Width { Zoom::Width } else { Zoom::PerRow(first.per_row) };
    let per_row = usize::from(first.per_row.max(1));
    let (mut failed, mut drawn_rows) = (Vec::new(), 0usize);
    let mut stdout = std::io::stdout().lock();
    for (row, chunk) in pages.chunks(per_row).enumerate() {
        let start = row.saturating_mul(per_row);
        let grid = fit::grid(zoom, pages.len(), start, size_of, area);
        let mut printed = Vec::new();
        for (slot, &page) in (0u16..).zip(chunk) {
            let drawn = doc
                .layout(page, grid.fit, grid.cols, grid.page_rows, cell, max_rows)
                .and_then(|layout| encode(&renderer.render(request(page, &layout, 0)), terminal.effort).map(|image| (layout, image)));
            match drawn {
                Ok((layout, image)) => {
                    if let Some(warning) = &image.warning {
                        note(&format!("warning: {warning}"));
                    }
                    let index = u64::try_from(start.saturating_add(usize::from(slot))).unwrap_or(u64::MAX);
                    printed.push(Printed { slot, index, layout, png: image.png });
                }
                Err(e) => failed.push(e),
            }
        }
        if printed.is_empty() {
            continue;
        }
        let mut out = Vec::new();
        // Rows of pages side by side have a row between them, as in the pager.
        if drawn_rows > 0 && grid.per_row > 1 {
            out.extend(std::iter::repeat_n(b'\n', usize::from(fit::ROW_GAP)));
        }
        drawn_rows = drawn_rows.saturating_add(1);
        print_row(&mut out, &printed, grid, terminal.output, first_id);
        stdout.write_all(&out).and_then(|()| stdout.flush()).map_err(CliError::Stdout)?;
    }
    if failed.is_empty() { Ok(()) } else { Err(format!("view: {}", failed.join("; ")).into()) }
}

/// A page of a printed row: its place in the row, its place in the list (which picks its image
/// id inside tmux), the cells it covers, and its image.
struct Printed {
    slot: u16,
    index: u64,
    layout: Layout,
    png: Vec<u8>,
}

/// Draw a row of printed pages, at least one, at the cursor where `grid` puts them, and leave the
/// cursor at the start of the line after it. A page alone in its row is drawn the way text is, the cursor and
/// the screen moving down with it, so a page taller than the window scrolls by. Pages side by side
/// go into a band of lines made first, each placed from the band's top with the cursor left
/// where it is; the band is never taller than the window, so going back up to it always works.
fn print_row(out: &mut Vec<u8>, row: &[Printed], grid: Grid, output: Output, first_id: u64) {
    let alone = grid.per_row <= 1;
    // Where a page goes in the band: across, and down to centre it in its place.
    let at = |p: &Printed| {
        let across = p.slot.saturating_mul(grid.cols.saturating_add(fit::GAP)).saturating_add(grid.cols.saturating_sub(p.layout.cols) / 2);
        if alone { (0, 0) } else { (across, grid.page_rows.saturating_sub(p.layout.rows) / 2) }
    };
    match output {
        Output::Direct if alone => {
            for p in row {
                kitty::show_png(out, &p.png, p.layout.cols, p.layout.rows);
                out.push(b'\n');
            }
        }
        Output::Direct => {
            let band = grid.page_rows;
            out.extend(std::iter::repeat_n(b'\n', usize::from(band)));
            cursor_by(out, band, 'A');
            for p in row {
                let (x, y) = at(p);
                out.push(b'\r');
                cursor_by(out, x, 'C');
                cursor_by(out, y, 'B');
                kitty::show_png_in_place(out, &p.png, p.layout.cols, p.layout.rows);
                cursor_by(out, y, 'A');
            }
            cursor_by(out, band, 'B');
            out.push(b'\r');
        }
        Output::Tmux(colour) => {
            let id = |p: &Printed| colour.id(first_id.wrapping_add(p.index));
            for p in row {
                kitty::transmit(out, id(p), &p.png, Wrap::Tmux);
                kitty::place_virtual(out, id(p), p.layout.cols, p.layout.rows, Wrap::Tmux);
            }
            // A line of text for each row of cells: every page's placeholder cells in its place.
            let lines = if alone { row.first().map_or(0, |p| p.layout.rows) } else { grid.page_rows };
            for line in 0..lines {
                let mut written = 0u16;
                for p in row {
                    let (x, y) = at(p);
                    let Some(cells) = line.checked_sub(y).filter(|r| *r < p.layout.rows) else { continue };
                    out.extend(std::iter::repeat_n(b' ', usize::from(x.saturating_sub(written))));
                    // The layout keeps rows and columns within the placeholders' table.
                    if kitty::placeholder_row(out, id(p), colour, cells, p.layout.cols) {
                        written = x.max(written).saturating_add(p.layout.cols);
                    }
                }
                out.push(b'\n');
            }
        }
    }
}

/// Move the cursor `n` cells (`code`: `A` up, `B` down, `C` right); nothing for 0, which the
/// terminal would take as 1.
fn cursor_by(out: &mut Vec<u8>, n: u16, code: char) {
    if n > 0 {
        out.extend_from_slice(format!("\x1b[{n}{code}").as_bytes());
    }
}

/// Where this run's image ids start ([`IdColor::id`]): mixed from the clock and the process id,
/// so runs in the same terminal are unlikely to draw over each other's images.
fn fresh_index() -> u64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    now.as_secs().wrapping_mul(1_000_000_007) ^ u64::from(now.subsec_nanos()) ^ u64::from(std::process::id()).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// The open document: the display copy the renderers read, and each page's displayed size.
struct Doc {
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    /// In points, after `/Rotate` and `/UserUnit`.
    sizes: Vec<(f32, f32)>,
    count: usize,
}

/// A page rendered for the terminal, as a PNG.
struct Image {
    png: Vec<u8>,
    /// Something the reader should know about the page, such as content left out.
    warning: Option<String>,
}

impl Doc {
    fn open(path: &str, password: Option<&str>) -> Result<Self, CliError> {
        // What `render` draws: the app's display copy, with the password the renderer takes.
        let display = displayed("view", path, read(path)?, password)?;
        let info = display.info.map_err(|e| format!("{path}: {e}"))?;
        let count = info.pages.len();
        if count == 0 {
            return Err(format!("{path}: no pages to show").into());
        }
        Ok(Self {
            bytes: display.bytes,
            config: RenderConfig { password: display.password.as_deref().map(Arc::from), reject_oversize: true, ..Default::default() },
            sizes: info.pages.iter().map(|p| (p.width, p.height)).collect(),
            count,
        })
    }

    /// Where `page` goes in `cols` × `rows` cells, never more than `max_rows` tall. Errors name the
    /// page.
    fn layout(&self, page: usize, fit: Fit, cols: u16, rows: u16, cell: Cell, max_rows: u16) -> Result<Layout, String> {
        let number = page.saturating_add(1);
        let (w, h) = self.sizes.get(page).copied().ok_or_else(|| format!("no page {number}"))?;
        fit::layout(w, h, cols, rows, cell, fit, max_rows).map_err(|e| format!("page {number}: {e}"))
    }

    /// A renderer on this thread, for pages printed one after another.
    fn renderer(&self) -> PageRenderer {
        PageRenderer::new(self.bytes.clone(), self.config.clone())
    }

    /// Render threads with the renderer's watchdog, for the pager: keys still work while pages
    /// render, and a page that takes too long is given up on.
    fn pool(&self, threads: usize) -> RenderPool {
        RenderPool::new(self.bytes.clone(), threads, self.config.clone())
    }
}

/// The render of `page` at `layout`'s scale, tagged so its result can be told from others.
fn request(page: usize, layout: &Layout, tag: u64) -> RenderRequest {
    RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale: layout.scale, tag, preview: false }
}

/// A rendered page as a PNG, or why there is none. Errors name the page.
fn encode(p: &RenderedPage, effort: PngEffort) -> Result<Image, String> {
    let warning = checked(p)?;
    Ok(Image { png: png(p, effort)?, warning })
}

/// Why a page failed to render, or what the reader should know about it, if anything: what
/// `render` warns about, in its words. Both name the page.
fn checked(p: &RenderedPage) -> Result<Option<String>, String> {
    let number = p.request.page.saturating_add(1);
    if let Some(e) = &p.error {
        return Err(format!("page {number}: {e}"));
    }
    // Each at most once: the renderer reports a warning once a page.
    let said: Vec<&str> = p
        .warnings
        .iter()
        .map(|w| match w {
            RenderWarning::ContentTruncated => "content past the page's safety budget was skipped",
            RenderWarning::ColourFallback => "a colour that could not be computed was painted in a fallback colour",
        })
        .collect();
    Ok((!said.is_empty()).then(|| format!("page {number}: {}", said.join("; "))))
}

/// A rendered page's pixels as a PNG. Errors name the page.
fn png(p: &RenderedPage, effort: PngEffort) -> Result<Vec<u8>, String> {
    let number = p.request.page.saturating_add(1);
    pdfcraft_engine::export::encode_png_with(p.width, p.height, &p.rgba, effort).map_err(|e| format!("page {number}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_file_is_the_token_that_is_not_an_option_value() {
        assert_eq!(file_arg(&args(&["a.pdf", "--pages", "2"])), Ok("a.pdf"));
        assert_eq!(file_arg(&args(&["--pages", "2", "a.pdf"])), Ok("a.pdf"));
        assert_eq!(file_arg(&args(&["--password", "a.pdf", "b.pdf"])), Ok("b.pdf"));
        // `--pager` takes no value.
        assert_eq!(file_arg(&args(&["--pager", "a.pdf"])), Ok("a.pdf"));
        assert!(file_arg(&args(&["--zoom", "2"])).is_err_and(|e| e.contains("missing file")));
        assert!(file_arg(&args(&["a.pdf", "b.pdf"])).is_err_and(|e| e.contains("one file at a time")));
        // A value left off the end is not a file.
        assert!(file_arg(&args(&["--pages"])).is_err());
    }

    #[test]
    fn nothing_written_out_can_start_an_escape_sequence() {
        // OSC 52 (a clipboard write), a title change, a kitty graphics command, 8-bit CSI and OSC.
        let hostile = "a\x1b]52;c;cHduZWQ=\x07b\x1b]0;title\x1b\\c\x1b_Ga=d\x1b\\d\u{9b}2J\u{9d}0;t\u{7}";
        let safe = printable(hostile);
        assert!(!safe.chars().any(char::is_control), "{safe:?}");
        assert!(safe.starts_with("a\u{FFFD}]52;c;cHduZWQ=\u{FFFD}b"), "{safe:?}");
        assert_eq!(printable("two\nlines\tand\r"), "two lines and ");
        assert_eq!(printable("rapport-été.pdf"), "rapport-été.pdf");
    }

    #[test]
    fn page_ids_follow_on_from_the_runs_start() {
        let first = fresh_index();
        let (a, b) = (IdColor::Indexed.id(first), IdColor::Indexed.id(first.wrapping_add(1)));
        assert_ne!(a, b);
        assert_ne!(a & 0xff, 0);
    }

    #[test]
    fn inside_tmux_the_client_terminal_decides() {
        let client = |termname: &str, rgb| Ok(tmux::Client { termname: termname.into(), rgb, cell: None });
        assert_eq!(tmux_colour(Protocol::Auto, &client("xterm-kitty", true)), Ok(IdColor::Rgb));
        assert_eq!(tmux_colour(Protocol::Auto, &client("xterm-ghostty", false)), Ok(IdColor::Indexed));
        let other = tmux_colour(Protocol::Auto, &client("xterm-256color", true));
        assert!(other.is_err_and(|e| e.contains("(xterm-256color)") && e.contains("--protocol kitty")));
        let unknown = tmux_colour(Protocol::Auto, &Err("tmux: not found".into()));
        assert!(unknown.is_err_and(|e| e.contains("tmux: not found")));
        // Trusted: whatever tmux says, or when it can't say.
        assert_eq!(tmux_colour(Protocol::Kitty, &client("xterm-256color", true)), Ok(IdColor::Rgb));
        assert_eq!(tmux_colour(Protocol::Kitty, &Err("tmux: not found".into())), Ok(IdColor::Indexed));
    }

    #[test]
    fn placeholders_limit_the_size_of_a_page() {
        assert_eq!(Output::Direct.limits(400), (400, u16::MAX));
        assert_eq!(Output::Tmux(IdColor::Rgb).limits(400), (297, 297));
        assert_eq!(Output::Tmux(IdColor::Rgb).limits(80), (80, 297));
    }

    #[test]
    fn more_than_a_page_on_a_terminal_opens_the_pager() {
        let auto = Paging::from_flags(false, false).unwrap();
        assert!(auto.pages(18, true));
        assert!(!auto.pages(1, true), "one page is printed, as less -F does");
        assert!(!auto.pages(18, false), "not on a terminal");
        assert!(Paging::from_flags(true, false).unwrap().pages(1, true));
        assert!(!Paging::from_flags(false, true).unwrap().pages(18, true));
        assert!(Paging::from_flags(true, true).is_err_and(|e| e.contains("not both")));
    }

    #[test]
    fn compression_follows_where_the_terminal_is() {
        assert_eq!(Compression::parse("auto"), Ok(Compression::Auto));
        assert!(Compression::parse("best").is_err_and(|e| e.contains("auto, fast or small")));
        // Over SSH, small PNGs; on this machine, fast ones, and temporary files may be tried.
        assert_eq!(Compression::Auto.choose(|| true), (PngEffort::Small, false));
        assert_eq!(Compression::Auto.choose(|| false), (PngEffort::Fast, true));
        // Chosen by hand: PNGs in the stream, and where the terminal is isn't even asked.
        assert_eq!(Compression::Fast.choose(|| panic!("asked")), (PngEffort::Fast, false));
        assert_eq!(Compression::Small.choose(|| panic!("asked")), (PngEffort::Small, false));
        // tmux's session knows better than a pane's environment, which may be from another attach.
        assert!(over_ssh(true, None));
        assert!(!over_ssh(false, None));
        assert!(!over_ssh(true, Some(false)));
        assert!(over_ssh(false, Some(true)));
    }

    #[test]
    fn protocol_names() {
        assert_eq!(Protocol::parse("auto"), Ok(Protocol::Auto));
        assert_eq!(Protocol::parse("kitty"), Ok(Protocol::Kitty));
        assert!(Protocol::parse("sixel").is_err_and(|e| e.contains("auto or kitty")));
    }

    /// Page 3 rendered with `warnings`, or failed with `error`.
    fn rendered(warnings: Vec<RenderWarning>, error: Option<&str>) -> RenderedPage {
        let request = RenderRequest { page: 2, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false };
        let rgba = pdfcraft_render::Pixels::from(Vec::new());
        RenderedPage { request, width: 0, height: 0, rgba, error: error.map(String::from), text: None, warnings, millis: 0 }
    }

    #[test]
    fn a_page_says_everything_the_renderer_warned_about() {
        assert_eq!(checked(&rendered(Vec::new(), None)), Ok(None));
        let fallback = "a colour that could not be computed was painted in a fallback colour";
        assert_eq!(checked(&rendered(vec![RenderWarning::ColourFallback], None)), Ok(Some(format!("page 3: {fallback}"))));
        assert_eq!(
            checked(&rendered(vec![RenderWarning::ContentTruncated, RenderWarning::ColourFallback], None)),
            Ok(Some(format!("page 3: content past the page's safety budget was skipped; {fallback}")))
        );
        // A page that failed says only that.
        assert_eq!(checked(&rendered(vec![RenderWarning::ColourFallback], Some("no page box"))), Err("page 3: no page box".into()));
    }
}
