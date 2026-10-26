# Showing pages in the terminal

`pdfcraft-cli view` draws PDF pages straight into the terminal, through the
[kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/). kitty, Ghostty
and WezTerm speak it, among others. It works on macOS, Linux and FreeBSD, and inside tmux.

```sh
pdfcraft-cli view report.pdf                    # page through it, like less
pdfcraft-cli view report.pdf --pages 1,3-5      # page through just these
pdfcraft-cli view report.pdf --zoom width       # as wide as the window
pdfcraft-cli view report.pdf --zoom 4           # four pages to a row
pdfcraft-cli view report.pdf --no-pager         # print every page and exit, like cat
pdfcraft-cli view secret.pdf --password PW
```

On a terminal, more than one page opens the pager: one page at a time, two side by side when
the window is wide enough, or zoomed out, rows of them. A single page (a one-page document, or `--pages 3`) is printed
at the cursor instead, and `view` exits, the way `less -F` prints text that fits.

Output to a file or a pipe takes `--protocol kitty` (without it, `view` stops with an error).
Then `view` doesn't page: it writes every page's escape codes one after another, and `cat` shows
them later in a terminal that speaks the protocol.

Printed pages are drawn in rows one under another and stay in the terminal's scrollback, the
same as text that `cat` prints. `--pages` takes the same list as `extract`: single pages and ranges, in
the order given. Without it, `view` shows the whole document.

The pages are the ones `render` and the app draw: comments without an appearance of their own are
drawn too, and a dynamic XFA form shows the pages its template lays out into, not the
placeholder page the file carries for viewers that can't.

To write an image file instead, use `render`:
`pdfcraft-cli render report.pdf --page 1 --out page.png`.

How `view` works inside (the transports, what the terminal keeps, what tmux loses and the numbers
behind each choice) is in [`apps/pdfcraft-cli/src/view/README.md`](../apps/pdfcraft-cli/src/view/README.md).

## Options

| Option | Meaning |
|---|---|
| `--pages 1,3-5` | the pages to show, in order (default: all of them) |
| `--zoom auto` | two pages side by side when the window is wide enough, otherwise one (the default) |
| `--zoom width` | one page as wide as the window; a tall page runs past the bottom |
| `--zoom 3` | that many pages to a row, 1 to 8, in as many rows as fit the window |
| `--pager` | always page, even through a single page |
| `--no-pager` | never page: print the pages and exit |
| `--compression auto` | on this machine, temporary files when the terminal takes them and `fast` otherwise; `small` over SSH (the default) |
| `--compression fast` | PNGs with the least work: pages ready sooner, about twice the bytes |
| `--compression small` | PNGs with the fewest bytes, for a slow link |
| `--protocol auto` | check that the terminal can show images first (the default) |
| `--protocol kitty` | skip the check and send the escape codes anyway, even to a file or a pipe |
| `--password PW` | the password of an encrypted document: the user or the owner password |

Printed without the pager, pages follow the zoom too, a row of them under another: `--zoom 3`
prints three to a row, `--zoom width` one to a row as wide as the window, and `auto` two to a row
when the window is wide enough, as the pager first shows them.

## Paging through a document

The pager shows pages on the alternate screen, like `less`. The shell's scrollback is left as it
was, and quitting returns to it. With `--pages`, the pager goes through those pages only, in
their order.

When the window is wide enough, two pages share it side by side, like an open book: pages 1–2,
then 3–4, and the keys that turn a page turn both. Two pages go side by side when each keeps at
least 80% of the size it would have alone, as portrait pages in a wide window do. Until you zoom,
the pager decides this again whenever the window changes size.

`-` zooms out: one more page to a row each time, in as many rows as fit the window, so zoomed
far out the screen is an overview of the document. It stops at 8 pages to a row, or before a page
would be narrower than 8 columns, and at as many pages as there are. `+` zooms in: one page fewer
to a row, and from one page, the page as wide as the window, which scrolls; that is as far in as
it goes. `=` and `_` work too: the same keys, with Shift or without.

Rows start the way a book's spreads do: at three to a row, pages 1–3, 4–6 and so on. The page
you are on stays on screen as you zoom, and the last row of the document stops at the bottom of
the screen, the way `less` stops at the last line. The page at the start of your row sets the
size of every page on screen, and a page is never made smaller to fit another row: at one page
to a row, a tall window shows landscape pages two or three at a time, one under another.

| Keys | Action |
|---|---|
| `space`, `PgDn` | down a screen; at the bottom of a page, on to the next page (or pages) |
| `b`, `PgUp` | up a screen; at the top of a page, back to the previous page (or pages) |
| `j`, `↓`, `Enter` | down a few rows (then on to the next page); with pages side by side, down a row of pages |
| `k`, `↑` | up a few rows (then back to the previous page); with pages side by side, up a row of pages |
| `n`, `→` | next page (or screen of pages) |
| `p`, `←` | previous page (or screen of pages) |
| `g`, `Home` | first page |
| `G`, `End` | last page |
| a number, then `Enter` or `g` | that page |
| `-`, `_` | zoom out: one more page to a row |
| `+`, `=` | zoom in: one page fewer to a row, then the page as wide as the window |
| `r`, `Ctrl-L` | render the pages again |
| `q`, `Esc`, `Ctrl-C` | quit |

The pager follows the window: after a resize, the pages are drawn again for the new size, as many
to a row as you zoomed to, or as the new size allows. Pages are rendered when they are shown, on a thread
of their own: the keys keep working while a slow page renders (the status line says it is
rendering), moving on cancels it, and the renderer's watchdog gives up on a page that takes too
long (20 seconds a try) and says so.

The pages around the one on screen are prepared ahead, so turning to them is instant. Jumping
further away waits for the page to render.

Quitting always puts the terminal back as it was, including after an error. A pager killed by a
signal (`kill`, a closed SSH session) can leave the terminal in raw mode; `reset` fixes that.

## Size and sharpness

Each page is rendered for the window it is shown in, at the terminal's own pixel density, so text
stays sharp on high-DPI screens. One row is left free for the shell prompt. Pages are never
rendered larger than 8192 pixels on a side or 16 megapixels in all (more than a 4K screen shows),
which bounds the memory one page can take, however its document shapes it.

The size in pixels usually comes with the window's size, as it does over SSH. Some ways in pass
on only rows and columns, such as `docker exec` and `kubectl exec`; then `view` asks the terminal
how big its cells are, once, when it starts, and assumes 10 × 20 pixel cells if the terminal
doesn't say. Inside tmux, tmux says. When there is no terminal to ask (output to a file with
`--protocol kitty`, no controlling terminal), `view` assumes an 80 × 24 window.

## On this machine or over SSH

When the terminal runs on the same machine, the pager hands it pages through temporary files, as
the protocol allows: kitty takes a page this way much faster than as a PNG inside the escape
codes. Each page's pixels go into a new file that only you can read, which the terminal
deletes as soon as it has read it. The files are in a directory of your own,
`tty-graphics-protocol-pdfcraft-<uid>`, inside `TMPDIR`, else `/dev/shm`, else `/tmp`. Any the
terminal leaves are removed when the pager ends, or, if it was killed, by the next one.

Whether the terminal reads files from this machine is found out by trying, at the start. Until it
does, and for good when it doesn't (a terminal without temporary files, or one on another
machine), pages travel as PNGs inside the escape codes. They also do when the temporary directory
isn't safe from other users, and when printing without the pager, so saved or piped output still
works.

A small PNG takes longer to make, and a fast one takes more bytes; the pixels are the same either
way. On the same machine, bytes are cheap and the time is what shows, so `view` compresses fast.
Over SSH, the bytes cross the network, so it compresses small and doesn't try temporary files. It
goes by `SSH_CONNECTION`, `SSH_CLIENT` and `SSH_TTY`, and inside tmux by how the tmux client
attached. `--compression fast` or `--compression small` overrides the guess, and sends PNGs even
on this machine.

## Which terminals work

`--protocol auto` trusts the environment for kitty (`TERM=xterm-kitty` or `KITTY_WINDOW_ID`) and
Ghostty (`TERM=xterm-ghostty` or `TERM_PROGRAM=ghostty`). Any other terminal is asked with the
query the protocol recommends, and has half a second to answer. A terminal that doesn't speak the
protocol gets an error that says so, and nothing is drawn.

Over SSH, the image is sent inside the escape codes themselves, so `view` works there too.

`view` fails when stdout isn't a terminal, unless `--protocol kitty` is given. Windows isn't
supported yet: Windows Terminal speaks sixel, not the kitty protocol. Sixel may come later, which
would also bring the terminals that speak only sixel.

## Inside tmux

`view` works inside tmux 3.3 and later when tmux runs in a terminal that speaks the protocol:

- tmux doesn't know about images, so the escape codes go through tmux's passthrough. tmux only
  lets them through when the pane's `allow-passthrough` option is `on` or `all`. The option lets
  any program in the pane send escape codes past tmux, so `view` never changes it. If it is off,
  `view` stops and says how to turn it on: for every pane, add `set -g allow-passthrough on` to
  `~/.tmux.conf`; for one pane, run the `tmux set -p -t <pane> allow-passthrough on` it shows.
- Pages are drawn with the protocol's *Unicode placeholders*: special characters that tmux stores
  as text, which the terminal draws as the image. The page scrolls with the pane, and survives
  switching windows and panes. Placeholders number at most 297 rows and columns, so inside tmux a
  page is never drawn larger than that.
- Inside tmux, `TERM` describes tmux, not the terminal, so `view` asks tmux which terminal it runs
  in. `xterm-kitty` and `xterm-ghostty` are trusted. For anything else, `view` refuses unless
  `--protocol kitty` is given.
- Pages turned while the pane is hidden (another window in front, or the window zoomed on another
  pane) arrive when it shows again: tmux throws away images sent to a hidden pane. A terminal that
  attaches to the session later gets the pages too.
- With more than one terminal showing the pane, pages go as PNGs, not temporary files: the first
  terminal to read a file deletes it before the others can.
- tmux throws output away when it decides the terminal can't keep up, images with it. `view`
  notices and sends the pages again. If tmux keeps dropping them, the status line says so, and
  `r` sends them again.

The terminal itself must support Unicode placeholders. kitty does. Other terminals may draw nothing
inside tmux even though they draw pages outside it.

## Errors

- A page number past the end of the document fails before anything is drawn.
- A page that can't be rendered is reported at the end. The other pages are still drawn, and
  `view` exits with an error.
- A page whose content runs past the renderer's safety budget is drawn without the rest of its
  content, with a warning, as in `render`.
- A colour the renderer can't compute (a colour space it can't read, a tint transform it can't
  evaluate) is painted in a fallback colour, with a warning, as in `render`.
- File names and messages are written with their control characters replaced, so neither a file
  name nor a document can send escape sequences (a clipboard write, a window title) to the
  terminal through them.
