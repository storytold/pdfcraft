//! Drawing from inside tmux. tmux keeps its own copy of the screen and knows nothing of images,
//! so `view` sends its escape codes through tmux's passthrough (`allow-passthrough`, `man tmux`)
//! and draws Unicode placeholders that tmux keeps as text (see `kitty.rs`). Inside tmux, `TERM`
//! names tmux, not the terminal, so tmux is asked which terminal its client is.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a `tmux` command may take before `view` gives up on it.
const TIMEOUT: Duration = Duration::from_secs(2);

/// Whether this process runs inside tmux: tmux sets `TMUX` in its panes.
pub(crate) fn inside() -> bool {
    std::env::var_os("TMUX").is_some_and(|v| !v.is_empty())
}

/// What tmux says about the terminal its client runs in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Client {
    /// `#{client_termname}`: the terminal's `TERM` outside tmux, such as `xterm-kitty`.
    pub(crate) termname: String,
    /// `RGB` is among `#{client_termfeatures}`: tmux passes 24-bit colours through unchanged.
    pub(crate) rgb: bool,
    /// `#{client_cell_width}` × `#{client_cell_height}`: the terminal's cell in pixels, when tmux
    /// knows it (tmux 3.4 and later).
    pub(crate) cell: Option<(u16, u16)>,
}

pub(crate) fn client() -> Result<Client, String> {
    tmux(&["display-message", "-p", "#{client_termname}|#{client_termfeatures}|#{client_cell_width}|#{client_cell_height}"])
        .map(|out| parse_client(&out))
}

/// `display-message -p` output for the format in [`client`]. Fields an older tmux leaves empty
/// are missing, never errors.
fn parse_client(out: &str) -> Client {
    let line = out.lines().next().unwrap_or_default();
    let mut fields = line.split('|');
    let termname = fields.next().unwrap_or_default().trim().to_string();
    let rgb = fields.next().unwrap_or_default().split(',').any(|f| f.trim() == "RGB");
    let mut pixels = || fields.next().and_then(|v| v.trim().parse::<u16>().ok()).filter(|v| *v > 0);
    let cell = pixels().zip(pixels());
    Client { termname, rgb, cell }
}

/// Whether tmux passes this pane's escape codes through: `allow-passthrough` must be `on` or
/// `all` (tmux 3.3 and later drop them otherwise). `view` only asks. The setting lets any program
/// in the pane write to the terminal past tmux, so it is the user's to change
/// ([`passthrough_off`]).
pub(crate) fn passthrough() -> Result<bool, String> {
    let pane = this_pane();
    let show = tmux(&[&["show-options", "-Ap"][..], &target(pane.as_deref()), &["allow-passthrough"]].concat())?;
    Ok(passthrough_allowed(&show))
}

/// What to tell the user when this pane's `allow-passthrough` is off: how to turn it on.
pub(crate) fn passthrough_off() -> String {
    passthrough_off_for(this_pane().as_deref())
}

/// [`passthrough_off`] for `pane`, which the command names when it is a pane id (`%3`): run from
/// the shell, tmux's current pane is the session's active one, which may be another.
fn passthrough_off_for(pane: Option<&str>) -> String {
    let pane_id = pane.filter(|p| p.strip_prefix('%').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())));
    let target = pane_id.map(|p| format!(" -t {p}")).unwrap_or_default();
    format!(
        "tmux's allow-passthrough is off for this pane, so tmux would drop the images. Turn it on for this pane with \
         `tmux set -p{target} allow-passthrough on`, or for every pane with `set -g allow-passthrough on` in ~/.tmux.conf \
         (it lets programs in the pane send escape codes past tmux)"
    )
}

/// The pane `view` runs in, as tmux names it in `TMUX_PANE`.
fn this_pane() -> Option<String> {
    std::env::var("TMUX_PANE").ok().filter(|pane| !pane.is_empty())
}

/// `-t pane`, or nothing (tmux's current pane) when the pane isn't known.
fn target(pane: Option<&str>) -> Vec<&str> {
    pane.map_or_else(Vec::new, |pane| vec!["-t", pane])
}

/// `show-options -Ap allow-passthrough` output, such as `allow-passthrough* off`; the `*` marks
/// a value inherited from the window or the global options.
fn passthrough_allowed(show: &str) -> bool {
    matches!(show.split_whitespace().last(), Some("on" | "all"))
}

/// The most clients looked at, and remembered.
const MAX_CLIENTS: usize = 64;

/// What tmux says about the pane `view` runs in and the clients attached.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// The pane's window, as `@3`.
    window: String,
    /// The window is zoomed on another pane, so this one doesn't show.
    zoomed_away: bool,
    clients: Vec<ClientReport>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ClientReport {
    /// `#{client_name}` and `#{client_pid}`: a client that attaches again is another one.
    who: String,
    /// `#{client_discarded}`: the bytes tmux has thrown away for it because its terminal fell
    /// behind. Escape codes go with them, images included.
    discarded: Option<u64>,
    /// The window it shows.
    window: String,
}

/// Ask tmux about the pane and its clients; `None` when it doesn't answer.
pub(crate) fn report() -> Option<Report> {
    let pane = this_pane();
    let args = [
        &["display-message", "-p"][..],
        &target(pane.as_deref()),
        &[
            "#{window_id}|#{window_zoomed_flag}|#{pane_active}",
            ";",
            "list-clients",
            "-F",
            "#{client_name}|#{client_pid}|#{client_discarded}|#{window_id}",
        ],
    ]
    .concat();
    tmux(&args).ok().and_then(|out| parse_report(&out))
}

/// The output of the commands in [`report`]: the pane's line, then a line per client (the first
/// [`MAX_CLIENTS`]).
fn parse_report(out: &str) -> Option<Report> {
    let mut lines = out.lines();
    let mut pane = lines.next()?.split('|').map(str::trim);
    let window = pane.next().filter(|w| !w.is_empty())?.to_string();
    let zoomed_away = pane.next() == Some("1") && pane.next() == Some("0");
    let clients = lines
        .take(MAX_CLIENTS)
        .filter_map(|line| {
            let mut fields = line.split('|').map(str::trim);
            let (name, pid) = (fields.next()?, fields.next()?);
            let discarded = fields.next().and_then(|d| d.parse().ok());
            let window = fields.next()?.to_string();
            Some(ClientReport { who: format!("{name}|{pid}"), discarded, window })
        })
        .collect();
    Some(Report { window, zoomed_away, clients })
}

/// Whether what the pager sends through tmux reaches every terminal showing the pane, from one
/// [`Report`] to the next. tmux passes escape codes through to every client showing the pane
/// and to none while it is hidden (`allow-passthrough on`), and what doesn't arrive never will.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    /// Every client seen showing the pane (the latest [`MAX_CLIENTS`]).
    seen: Vec<Seen>,
}

#[derive(Debug)]
struct Seen {
    who: String,
    /// Whether it showed the pane at the last report, and how many images had been sent then.
    showing: bool,
    sent: u64,
    discarded: Option<u64>,
}

/// What a report means for the images sent.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct News {
    /// How many terminals show the pane: none while it is hidden.
    pub(crate) showing: usize,
    /// One of them may lack images: it is new (or attached again: the terminal has dropped what
    /// it had), or it shows the pane again after images went out while it didn't.
    pub(crate) missed: bool,
    /// tmux threw output away for one of them.
    pub(crate) discarded: bool,
}

impl Watch {
    /// Take in `report`, `sent` images having been sent so far.
    pub(crate) fn update(&mut self, report: &Report, sent: u64) -> News {
        let showing: Vec<&ClientReport> =
            if report.zoomed_away { Vec::new() } else { report.clients.iter().filter(|c| c.window == report.window).collect() };
        let mut news = News { showing: showing.len(), ..News::default() };
        for client in &showing {
            match self.seen.iter_mut().find(|s| s.who == client.who) {
                Some(seen) => {
                    // Not showing the pane at the last report, with images sent since.
                    news.missed |= !seen.showing && seen.sent < sent;
                    news.discarded |= matches!((seen.discarded, client.discarded), (Some(before), Some(now)) if now > before);
                    seen.discarded = client.discarded;
                    seen.sent = sent;
                }
                None => {
                    news.missed |= sent > 0;
                    if self.seen.len() >= MAX_CLIENTS {
                        self.seen.remove(0);
                    }
                    self.seen.push(Seen { who: client.who.clone(), showing: true, sent, discarded: client.discarded });
                }
            }
        }
        for seen in &mut self.seen {
            seen.showing = showing.iter().any(|c| c.who == seen.who);
        }
        news
    }
}

/// Whether the tmux client was attached over SSH, by the session's `SSH_CONNECTION`: tmux
/// updates it at every attach (`update-environment`), while the pane's own environment dates
/// from when the pane started. `None` when tmux doesn't say.
pub(crate) fn attached_over_ssh() -> Option<bool> {
    tmux(&["show-environment", "SSH_CONNECTION"]).ok().and_then(|out| parse_ssh(&out))
}

/// `show-environment SSH_CONNECTION` output: `SSH_CONNECTION=…` when the client came over SSH,
/// `-SSH_CONNECTION` when it didn't (tmux removed it at the attach).
fn parse_ssh(out: &str) -> Option<bool> {
    let line = out.lines().next()?.trim();
    if line.starts_with("SSH_CONNECTION=") {
        Some(true)
    } else if line == "-SSH_CONNECTION" {
        Some(false)
    } else {
        None
    }
}

/// Run `tmux` with `args` and return its output, or say why not. Bounded however tmux behaves:
/// [`TIMEOUT`] covers both its exit and the end of its output (a process it left behind could
/// keep the pipes open), and at most [`MAX_OUTPUT`] bytes of each pipe are read.
fn tmux(args: &[&str]) -> Result<String, String> {
    let what = || format!("tmux {}", args.join(" "));
    let no_answer = || format!("{}: no answer within {} s", what(), TIMEOUT.as_secs());
    let deadline = Instant::now() + TIMEOUT;
    let mut child = Command::new("tmux")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", what()))?;
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let (sender, receiver) = std::sync::mpsc::channel();
    // A reader left blocked past the deadline ends with the process.
    let reader = std::thread::Builder::new().spawn(move || {
        let _ = sender.send((read_capped(stdout), read_capped(stderr)));
    });
    if let Err(e) = reader {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("{}: {e}", what()));
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(no_answer());
            }
            Err(e) => return Err(format!("{}: {e}", what())),
        }
    };
    let (out, err) = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())).map_err(|_| no_answer())?;
    if !status.success() {
        return Err(format!("{}: {}", what(), String::from_utf8_lossy(&err).trim()));
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The most bytes read from each of tmux's pipes: its answers here are a line.
const MAX_OUTPUT: u64 = 64 * 1024;

/// Up to [`MAX_OUTPUT`] bytes from `pipe`, until it closes.
fn read_capped(pipe: Option<impl Read>) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(pipe) = pipe {
        let _ = pipe.take(MAX_OUTPUT).read_to_end(&mut bytes);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_names_its_terminal_colours_and_cells() {
        // As tmux 3.5a reports a kitty 0.45 client on a Retina display.
        let kitty = parse_client("xterm-kitty|bpaste,ccolour,clipboard,cstyle,focus,RGB,title|18|46\n");
        assert_eq!(kitty, Client { termname: "xterm-kitty".into(), rgb: true, cell: Some((18, 46)) });
        let plain = parse_client("xterm-256color|bpaste,title|0|0\n");
        assert_eq!(plain, Client { termname: "xterm-256color".into(), rgb: false, cell: None });
        // An older tmux leaves the formats it doesn't know empty.
        assert_eq!(parse_client("xterm-ghostty|||\n"), Client { termname: "xterm-ghostty".into(), rgb: false, cell: None });
        assert_eq!(parse_client("xterm-kitty|RGB|18|\n").cell, None);
        assert_eq!(parse_client(""), Client::default());
        assert_eq!(parse_client("no separator").termname, "no separator");
        assert_eq!(parse_client("a|b|99999|x|more").cell, None);
    }

    /// A report: the pane in window `@1` (zoomed on another pane when `zoomed_away`), and clients
    /// `(who, discarded, window)`.
    fn report(zoomed_away: bool, clients: &[(&str, u64, &str)]) -> Report {
        let clients =
            clients.iter().map(|(who, d, w)| ClientReport { who: format!("{who}|1"), discarded: Some(*d), window: w.to_string() }).collect();
        Report { window: "@1".into(), zoomed_away, clients }
    }

    #[test]
    fn tmux_reports_the_pane_and_its_clients() {
        // As tmux 3.5a answers: the pane's window, not zoomed; two clients, the second in a
        // grouped session showing another window.
        let out = "@0|0|1\n/dev/ttys031|4242|0|@0\n/dev/ttys033|4250|512|@1\n";
        let report = parse_report(out).unwrap();
        assert_eq!((report.window.as_str(), report.zoomed_away), ("@0", false));
        assert_eq!(report.clients[1], ClientReport { who: "/dev/ttys033|4250".into(), discarded: Some(512), window: "@1".into() });
        assert!(parse_report("@0|1|0\n").unwrap().zoomed_away);
        assert!(!parse_report("@0|1|1\n").unwrap().zoomed_away, "zoomed on this pane");
        // No client: nothing discarded to tell. Lines short of a window are no clients.
        assert_eq!(parse_report("@0|0|1\n/dev/ttys031|1||@0\nbroken\n").unwrap().clients.len(), 1);
        assert_eq!(parse_report(""), None);
        assert_eq!(parse_report("|0|1"), None);
        let many = format!("@0|0|1\n{}", "c|1|0|@0\n".repeat(1000));
        assert_eq!(parse_report(&many).unwrap().clients.len(), MAX_CLIENTS);
    }

    #[test]
    fn a_terminal_that_may_lack_images_is_noticed() {
        let mut watch = Watch::default();
        // One terminal from the start, before any image.
        assert_eq!(watch.update(&report(false, &[("a", 0, "@1")]), 0), News { showing: 1, missed: false, discarded: false });
        assert!(!watch.update(&report(false, &[("a", 0, "@1")]), 5).missed);
        // It shows another window, and images go out: back, it lacks them.
        assert_eq!(watch.update(&report(false, &[("a", 0, "@2")]), 5).showing, 0);
        assert_eq!(watch.update(&report(false, &[("a", 0, "@2")]), 7).showing, 0);
        assert!(watch.update(&report(false, &[("a", 0, "@1")]), 7).missed);
        // Away and back with nothing sent meanwhile: it has everything.
        watch.update(&report(false, &[("a", 0, "@2")]), 7);
        assert!(!watch.update(&report(false, &[("a", 0, "@1")]), 7).missed);
        // Zoomed on another pane, the pane is hidden all the same.
        assert_eq!(watch.update(&report(true, &[("a", 0, "@1")]), 7).showing, 0);
        assert!(!watch.update(&report(false, &[("a", 0, "@1")]), 7).missed);
        // A second terminal comes: it has none of the images.
        assert_eq!(watch.update(&report(false, &[("a", 0, "@1"), ("b", 0, "@1")]), 7), News { showing: 2, missed: true, discarded: false });
        // tmux throws output away for one of them.
        assert!(watch.update(&report(false, &[("a", 0, "@1"), ("b", 9, "@1")]), 8).discarded);
        assert_eq!(watch.update(&report(false, &[("a", 0, "@1"), ("b", 9, "@1")]), 8), News { showing: 2, missed: false, discarded: false });
    }

    #[test]
    fn the_session_says_whether_its_client_came_over_ssh() {
        // As tmux 3.5a answers for a client attached locally.
        assert_eq!(parse_ssh("-SSH_CONNECTION\n"), Some(false));
        assert_eq!(parse_ssh("SSH_CONNECTION=192.0.2.7 50122 192.0.2.1 22\n"), Some(true));
        assert_eq!(parse_ssh(""), None);
        assert_eq!(parse_ssh("SSH_CONNECTIONS=x"), None);
    }

    #[test]
    fn passthrough_is_allowed_when_on_or_all() {
        assert!(passthrough_allowed("allow-passthrough on\n"));
        assert!(passthrough_allowed("allow-passthrough* all\n"));
        assert!(!passthrough_allowed("allow-passthrough* off\n"));
        assert!(!passthrough_allowed(""));
        assert!(!passthrough_allowed("unknown option: allow-passthrough"));
    }

    #[test]
    fn passthrough_off_says_how_to_turn_it_on_for_this_pane() {
        assert!(passthrough_off_for(Some("%12")).contains("`tmux set -p -t %12 allow-passthrough on`"));
        assert!(passthrough_off_for(None).contains("`tmux set -p allow-passthrough on`"));
        // Anything but a pane id is left out of a command the user may paste.
        for pane in ["%", "%1; rm -rf ~", "12", ""] {
            assert!(passthrough_off_for(Some(pane)).contains("`tmux set -p allow-passthrough on`"), "{pane:?}");
        }
    }
}
