# Developing PdfCraft

Working instructions for agents and contributors are in `AGENTS.md` and `CLAUDE.md`. This page
collects what the desktop app reads from its environment and where it writes its diagnostics.

## Logs

The desktop app writes its `log` records to standard error and to `logs/pdfcraft.log` in its
settings folder, next to `app.ron`: Linux and FreeBSD `~/.local/share/pdfcraft/logs/` (or
`$XDG_DATA_HOME/pdfcraft/logs/`), macOS `~/Library/Application Support/PdfCraft/logs/`, Windows
`%APPDATA%\PdfCraft\data\logs\`. In portable mode (a `portable.txt` file next to the executable, as
the Windows portable zip ships) the settings folder is `PdfCraftData\` beside the executable, and
crash recovery and new digital IDs go there too. A start from a desktop menu, Finder or the Start
menu has no terminal, so this file is what to attach to a bug report: a failed autosave, a page that
would not render, a render worker that could not start and the report of an internal error all land
there.
Each launch moves the previous log to `pdfcraft.1.log` (and that one to `pdfcraft.2.log`), so the
log of a run that crashed survives the next start. The file stops growing at 16 MiB. `--version`
writes no file.

By default PdfCraft's own crates (`pdfcraft*`) log at `info` and everything else at `warn`.
`RUST_LOG` replaces that with env_logger-style directives, for example `RUST_LOG=debug`,
`RUST_LOG=warn,pdfcraft_render=trace` or `RUST_LOG=info,wgpu_core=warn`; a directive ending in `*`
covers every target starting with it (`pdfcraft*=debug`). The logger is
`apps/pdfcraft/src/logging.rs`. It never records the control-channel token or document passwords.

## Environment variables

| Variable | Effect |
|---|---|
| `RUST_LOG` | Log levels for standard error and the log file (see [Logs](#logs)) |
| `RUST_BACKTRACE` | `1` adds a backtrace to the report of an internal error |
| `XDG_DATA_HOME` | Linux/FreeBSD: base of the settings folder (`pdfcraft/`), the log folder and crash recovery |
| `WGPU_POWER_PREF` | GPU choice; by default PdfCraft prefers the low-power (integrated) GPU |
| `WGPU_BACKEND` | Graphics backend; by default Windows uses Direct3D 12, falling back to OpenGL |
| `CRAFT_FONTS_DIR` | Build time: a [craft-fonts](https://github.com/storytold/craft-fonts) checkout to embed (Japanese fonts) |
| `PDFCRAFT_SYSTEM_FONTS` | `0` stops the desktop app from using an installed font for characters its embedded fonts lack (`cargo xtask screenshots` sets it) |

## Several windows

PdfCraft can show one document in several windows (Window ▸ New window, or a right-click on a
tab). They share the document, its undo history and its unsaved state; each has its own page,
zoom, selection and panels. With the control channel running, every window can be driven:

```bash
cargo run -p pdfcraft -- --control /tmp/pc.json some.pdf &
cargo run -p pdfcraft-cli -- ui --control /tmp/pc.json command id=window.new_view
cargo run -p pdfcraft-cli -- ui --control /tmp/pc.json windows
cargo run -p pdfcraft-cli -- ui --control /tmp/pc.json state --window 1
cargo run -p pdfcraft-cli -- ui --control /tmp/pc.json set key=zoom value=200 --window 1
cargo run -p pdfcraft-cli -- ui --control /tmp/pc.json screenshot --window 1 --out /tmp/w1.png
cargo run -p pdfcraft-cli -- ui --control /tmp/pc.json window_move_tab doc_index=0
cargo run -p pdfcraft-cli -- ui --control /tmp/pc.json window_close --window 1
```

Without `--window`, a request goes to the window that has the focus. `inspect`, `click`, `drag`,
`type` and `key` still work on the main window. In tests, windows are drawn inside the main one
(egui's embedded viewports), so `egui_kittest` can click in all of them; see
`crates/ui-egui/tests/windows.rs`.
