# `pdfcraft-cli view`: how it works

`view` draws PDF pages in the terminal through the
[kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/), directly or from
inside tmux. Usage and behaviour are in [`docs/cli-view.md`](../../../../docs/cli-view.md); this
page is for working on the code: how pages get to the terminal, what is kept there, what can go
wrong on the way, and why each choice was made. The numbers were measured on the macOS laptop it
was developed on (kitty 0.45, tmux 3.5a), by black-box observation. The same probes, run again
against kitty 0.49.2, found the same behaviour: temporary files read and deleted, directly and
through tmux, the same answers, the 320 MB quota and the cell size.

## Modules

| File | What it does |
|---|---|
| `mod.rs` | Arguments, the checks before drawing, printing pages without the pager (`--no-pager`, a single page), choosing PNG compression and whether to try temporary files |
| `detect.rs` | Whether the terminal speaks the protocol, from the environment and its answer to a query, and the terminal's answer about its cell size. Pure |
| `tty.rs` | `/dev/tty`: size, raw mode, and reads with a timeout through a reader thread (on macOS, `poll(2)` on `/dev/tty` answers `POLLNVAL` at once) |
| `keys.rs` | Bytes from the terminal into keys, and the terminal's answers to graphics commands. Pure, total over any bytes |
| `fit.rs` | The render scale and the cells a page covers. Pure arithmetic |
| `kitty.rs` | The protocol's escape codes, and the Unicode placeholders used inside tmux. Pure |
| `tmux.rs` | Asking tmux: its client's terminal, `allow-passthrough`, SSH, and who shows the pane |
| `pager.rs` | The pager: navigation (`Nav`, no IO), rendering and sending ahead, what is kept in the terminal, redraws |
| `files.rs` | Temporary files for a terminal on this machine, on a thread of their own |

`view` is Unix only (macOS, Linux, FreeBSD): it needs `/dev/tty`. Nothing in it is `unsafe`.

Sixel isn't supported yet, and is a possible later addition. It is DEC's older image format, the
one Windows Terminal speaks (as do xterm, foot, WezTerm and others), so it is the way to bring
`view` to Windows and to more terminals. It would be a second output beside `kitty.rs` more than a
change to it: sixel keeps no images in the terminal by id, and has no temporary files and no
Unicode placeholders, so every page turn sends the page's pixels again, and pictures go through a
palette (commonly 256 colours). On Windows, `tty.rs` would also need the console's own API for raw
mode and the window size, through a crate, since the workspace forbids `unsafe`.

## Getting pages to the terminal

A rendered page is premultiplied RGBA. The protocol offers three ways to send it:

| Way | Escape code | Used |
|---|---|---|
| PNG in the stream | `a=t,f=100,t=d`: base64 in 4096-byte chunks | Over SSH, when printing, and until the terminal takes a temporary file |
| Raw pixels in a temporary file | `a=t,f=32,t=t`: only the file's path, base64 | The pager, on a terminal on this machine that takes files |
| Shared memory | `t=s` | Never: it needs `mmap`, and the workspace forbids `unsafe` |

Temporary files are much faster for the terminal. kitty takes a full-window page in about 10 ms
from a file, against 52 ms as a PNG in the stream (64–66 ms through tmux). Nine pages sent back to
back through tmux took 495 ms as PNGs and 55–90 ms as files. A PNG also has to be made first: on
the same laptop, 35 ms for a text page and 110 ms for a full-screen page of pictures at the
smaller compression. Without that, everything `view` does between a key and the escape codes
takes under a millisecond; the render takes about 50 ms for a text page.

`--compression auto` (the default) picks the way (`Compression::choose` in `mod.rs`):

| Where the terminal is | PNG effort | Temporary files |
|---|---|---|
| This machine | fast (about twice the bytes for a text page and a third more for pictures, much less time) | tried by the pager |
| Over SSH (`SSH_CONNECTION`, `SSH_CLIENT`, `SSH_TTY`; inside tmux, `tmux show-environment SSH_CONNECTION`, which tmux updates at each attach) | small | never |

`--compression fast` or `small` sends PNGs, without trying files.

## Temporary files (`files.rs`)

- **The probe.** At the start, the pager sends a one-pixel image through a file. A terminal on
  this machine reads it and deletes it, as the protocol has it delete temporary files once read.
  Until it does, pages go as PNGs, and for good if it hasn't within 2 s.
- **Where.** `TMPDIR`, else `/dev/shm`, else `/tmp`, and in it a directory of the user's own,
  `tty-graphics-protocol-pdfcraft-<uid>`. The terminal deletes only files with
  `tty-graphics-protocol` in their path. The directory must be the user's, mode 0700 and not a
  symlink. The directory around it must be the user's or root's, and sticky if others can write
  to it, so no one else can move it away. Otherwise there are no files.
- **Each file** is `<pid>-<n>`, created new with mode 0600 (never an existing file, nor through a
  symlink), and written as straight RGBA in 256 KiB chunks.
- **A file the terminal leaves** is removed after 5 s, and pages go as PNGs from then on: the
  terminal doesn't take files after all, or the escape code never reached it. At most 8 files wait
  at once; pages go as PNGs until the terminal catches up.
- **Cleaning up.** The pager removes its files when it ends. A killed pager leaves them, and a
  later run removes them: files named `<pid>-<n>`, older than 60 s, whose process is gone
  (`kill(pid, 0)` says `ESRCH`), looking through at most 10,000 entries. Only the private
  directory is ever listed, never `TMPDIR` itself: on one macOS account that held 326,191 entries
  and took 26 s to list cold, with the pager on a black screen meanwhile.
- **A thread of their own** (`Files`). Every file operation runs on it, and the pager waits at
  most 1 s for an answer. After one that doesn't come (a hung network mount), files are over for
  the run: pages go as PNGs, and the pages whose files the terminal hadn't taken yet are sent
  again. The keys keep working throughout. At the end, once the terminal is put back, the pager
  waits up to 1 s more for the thread to remove its files, stalled or not: a loaded machine
  makes a thread slow far more often than a mount hangs it.

## The size of a cell

Pages are rendered for the pixels they cover, so `view` needs the cell size in pixels. The tty
gives it with the window's size (`TIOCGWINSZ`), and SSH carries it. `docker exec` and
`kubectl exec` carry only rows and columns, so the pixels read 0. Taking 10 × 20 px cells then
rendered a page for a kitty with 18 × 46 px cells at under half its resolution, upscaled, and
22% too narrow. So, with no pixels, `view` asks the terminal once at the start (`ask_cell` in
`mod.rs`): `ESC [ 16 t`, which xterm's control sequences define, and kitty answers with
`ESC [ 6 ; <height> ; <width> t` in about 3 ms. The query is followed by the device attributes
query, which every terminal answers, so one that doesn't know the first ends the wait at once;
then the 10 × 20 assumed stands. An answer arriving late reaches the pager as an unknown CSI
sequence, which is no key, even cut in two (see the terminal's answers below). Inside tmux,
`#{client_cell_width}` and `#{client_cell_height}` give the cell instead. The answer isn't asked
again, so changing the font size in a session that reports no pixels shows only on the next
start.

## Directly, or inside tmux

Directly, the pager transmits each page as an image of its own and shows it with a placement
(`a=p`), the cursor left where it is (`C=1`). Each place on screen has a placement id (`p=1`, and
`p=2` for the second of two pages side by side), so a page given twice shows twice. A page that
leaves the screen loses its placement (`a=d,d=i`) but keeps its data, so showing it again is only
a placement.

Inside tmux, tmux keeps its own copy of the screen and knows nothing of images:

- **Passthrough.** Every escape code goes inside `ESC P tmux; … ESC \`, with each ESC doubled.
  tmux passes it on only when the pane's `allow-passthrough` is `on` or `all`. `view` only asks
  (`show-options -Ap`): the option lets any program in the pane write to the terminal past tmux,
  so it is the user's. When it is off, `view` stops with the command that turns it on, naming the
  pane: run from the shell without `-t`, tmux sets it on the session's active pane, which may be
  another. Output to a file only gets a warning, since it may be shown where it is on.
- **Unicode placeholders.** A page is a virtual placement (`U=1`) plus rows of U+10EEEE cells that
  tmux stores as text, so pages scroll with the pane and survive window switches. The image id is
  in the cells' foreground colour: 24-bit when tmux reports `RGB` for the terminal, the 256-colour
  palette otherwise, with the id's high byte in a third diacritic. Only the first cell of each row
  carries the row and column diacritics, as the protocol allows; full marks made a screenful big
  enough for tmux to throw output away. Placeholders number at most 297 rows and columns, so
  pages inside tmux are never bigger.
- **The terminal.** `TERM` names tmux, so `view` asks tmux for its client's terminal
  (`#{client_termname}`) and cell size (`#{client_cell_width}`, `#{client_cell_height}`).

## What is kept in the terminal (`pager.rs`)

The pager works ahead. Two render threads render the pages on screen first, then the next two
screens and the one before. Each page is sent as soon as it is ready, so turning to it is only a
placement. The pages on screen stay up until the next ones are ready, so the screen never goes
blank in between.

The terminal keeps at most 12 pages, and 192 MiB of pixels (`MAX_SENT`, `BUDGET`). Rendering
ahead stops at the first page that wouldn't fit, and the least recently shown pages not on screen
are deleted (`a=d,d=I`) to make room. The byte budget matters. kitty keeps 320 MB of images per
screen (the protocol's example quota), and past it, it deletes nearly every image without a
placement at once, not just the oldest. Measured in a hidden kitty with raw images:

| Images sent | What kitty kept |
|---|---|
| 13 × 24.6 MB (319 MB) | all 13 |
| 14 × 24.6 MB (344 MB) | 1 (3 when two had placements: those survive) |
| 30 × 12.1 MB (364 MB) | 3 |

At 12 pages and no byte budget, fit-width pages on a 5K window (about 64 MB each) went past the
quota: kitty dropped 8 of the 12 pages the pager kept, and turning back showed them black.

When kitty drops a page anyway (another program's images, a terminal with a smaller quota),
placements tell. They ask for errors (`q=1`; every other code says `q=2`, no answers), and kitty
answers a placement of an image it doesn't have with `ESC _ G i=<id>,p=<p>;ENOENT:… ESC \`.
`keys.rs` turns that into `Key::Missing`, and the pager forgets the page, renders and sends it
again, and halves its budget, at most 8 times (`MAX_RESENDS`; `r` still sends everything again).
kitty takes a temporary file before it reads the next escape code, so a placement right after
one never gets this answer by mistake.

Any other answer is dropped. An answer must never be read as keys: its `ESC` would be the Esc
key, which quits, and its letters keys. One cut off between reads waits for the rest, up to 512
bytes and 1 s, and is dropped whole if the rest doesn't come. So does any escape sequence cut off
past its `ESC`, a late cell size or a key over SSH alike. A rest that comes later than that is
read as keys: no wait could tell it from typing, and terminals send each reply in one write. Only
an `ESC` alone, with nothing after it for 50 ms, is the Esc key. At the end, the pager waits up to 100 ms after its last placements,
so no late answer lands on the shell's command line.

## What tmux loses, and how the pager notices

tmux delivers what goes through passthrough once, to the terminals showing the pane at that
moment, and never again: it redraws its own text, not images. The pager asks tmux about the pane
150 ms after it writes, and every second anyway, in one command:

```sh
tmux display-message -p -t "$TMUX_PANE" '#{window_id}|#{window_zoomed_flag}|#{pane_active}' \; \
     list-clients -F '#{client_name}|#{client_pid}|#{client_discarded}|#{window_id}'
```

`tmux::Watch` keeps, for every client seen showing the pane, how many images had been sent when it
last did. The pager acts on what changed:

| What tmux says | What happened | What the pager does |
|---|---|---|
| No client shows the pane (another window, or zoomed on another pane) | With `allow-passthrough on`, tmux throws passthrough away | Sends nothing until the pane shows; removes waiting files without giving files up |
| A client shows it again after images went out without it | It missed them | Sends everything again |
| A client it hasn't seen (new, or attached again: the terminal clears the alternate screen's images when tmux returns to it) | It has none of the images | Sends everything again |
| More than one client shows it | Each gets every escape code, and the first to read a file deletes it | No more files; pages go as PNGs |
| A client's `#{client_discarded}` rose | tmux threw output away because the terminal fell behind | Sends everything again, at most 8 times |

A client is its `client_name` and `client_pid`, so attaching again from the same tty is a new
client. Temporary files start only inside a tmux that reports exactly one client showing the
pane.

## The terminal's state

The pager runs on the alternate screen, in raw mode, with the cursor hidden and line wrap off.
`Screen` restores all of it when it drops, on return, on an error through `?`, and while a panic
unwinds; nothing else is printed to stderr meanwhile (`QUIET`). The images are deleted before
that, and the temporary files removed after it, so a slow filesystem can't hold the terminal
in raw mode. File names, error messages and warnings are written with their control characters
replaced (`printable`), so a document can't send escape sequences through them.

When it will page, `view` takes raw mode before it asks the terminal anything (whether it speaks
the protocol, its cell size), and keeps it until the pager ends, never back in canonical mode
between the two. On FreeBSD a read begun in canonical mode stays a line read after raw mode
returns: had the reader thread begun its next read in such a gap, no key would reach the pager
until Enter. Found by CI on FreeBSD, where two pager tests hung after `q`; the race shows there
only.

## Tests

- **Unit tests** in each module, among them property tests for the key decoder and the fit.
- **`tests/it/view.rs`**: printing, through a pipe with `--protocol kitty`.
- **`tests/it/view_pager.rs`**: the pager on a pseudo-terminal from script(1), keys in through its
  stdin. It plays a terminal that takes temporary files (it reads each file named, checks its
  size, deletes it) or one that doesn't, and sends kitty's answers.
- **`tests/it/view_tmux.rs`**: the pager in a private tmux server (`tmux -L`, killed at the end), its
  clients on script(1) pseudo-terminals. It covers flips, a hidden pane, and a second client.

The terminal tests skip with a warning where tmux or script(1) can't run, so CI machines without
them pass.

To try a real kitty without touching your own, run `kitty --start-as=hidden --config NONE` with
`TMUX` and `TMUX_PANE` unset (an inherited `TMUX` makes `view` think it is inside your tmux: it
asks your tmux server about your pane, and wraps its escape codes for a tmux that isn't there),
and use a private `tmux -L <name>` server for tmux.

## Clean room

Behaviour comes from the kitty graphics protocol specification, the tmux manual and black-box
observation of terminals and of other programs' output. Never read the source of another
terminal-graphics program or PDF viewer (kitty, tdf, timg, chafa, viu, MuPDF and so on); most are
GPL or AGPL (`AGENTS.md` §2).
