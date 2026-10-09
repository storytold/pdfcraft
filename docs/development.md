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
| `PDFCRAFT_SYSTEM_FONTS` | `0` disables both runtime UI fallbacks (`cargo xtask screenshots` sets it); no effect on fonts embedded in PDFs |

## Installed interface fonts

On Windows, the desktop interface tries installed Microsoft YaHei, SimHei and SimSun for
Chinese UI text and file names, in every interface language. It validates each candidate's
font collection face and several Simplified Chinese glyphs before loading it. The existing
Arabic-script fallback and the macOS/Linux candidates are unchanged. If no suitable Chinese
font is installed, the embedded fallback stack remains usable, with replacement glyphs for
characters it cannot cover. This change does not add a Chinese fallback on macOS, Linux or web.

Both runtime fonts are read once, capped at 32 MiB each and 16 collection faces, and used
only by egui. No system font file is copied, converted, embedded in a PDF, committed, downloaded
or included in the installer. Use fonts legally installed on the user's machine; Microsoft's
[font redistribution FAQ](https://learn.microsoft.com/en-us/typography/fonts/font-faq) permits
Windows applications to use system-wide Windows fonts for interface display, while generally
prohibiting redistribution without a separate licence. Other fonts retain their own terms.

Published screenshots must use `PDFCRAFT_SYSTEM_FONTS=0`, as required by `AGENTS.md` §1.4.
For Windows regression validation with an installed Chinese font, set
`PDFCRAFT_TEST_REQUIRE_CJK=1` and run
`cargo test -p pdfcraft-ui-egui --test chinese_system_font -- --nocapture`.
The test skips when that font is absent unless the explicit validation flag is set. It checks
every non-ASCII translated character in `zh-hans.tsv`, Chinese file names in both UI font orders,
all four interface families, and retention of the existing Arabic fallback. Run the same test
in a separate process with `PDFCRAFT_SYSTEM_FONTS=0` to verify the publication opt-out.
