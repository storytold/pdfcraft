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

## GPU surface limits

The desktop renderer enables the selected adapter's supported 2D texture extent, while keeping
egui-wgpu's other device requirements. Restored windows on mixed-DPI monitors can exceed its
default 8192-pixel extent even when the GPU supports a larger surface (#577). The enabled extent
never exceeds the adapter's capability. A window larger than even that (restored at another
monitor's scale, or stretched across monitors) gets a surface fitted within the limit and is drawn
at a correspondingly lower scale instead of panicking in `Surface::configure` (vendored egui-wgpu,
`surface_fit`; see `vendor/README.md`). On DX12, PdfCraft's Windows default, the surface is
stretched over the window: softer, but complete and lined up with the pointer. wgpu's GL backend
copies it unscaled into a corner of the window instead.

## Renderer fallback

PdfCraft draws with wgpu, and starts again with eframe's own OpenGL renderer (glow) when wgpu fails
before the app is created: no adapter, no device, or a window surface the device can't configure.
The last of these was a panic, not an error, because wgpu's default error handler panics and
eframe configures the surface before any app code runs; a 2015 Intel GPU whose driver timed out in
`Surface::configure` on wgpu's GL backend closed PdfCraft at launch (#519). Vendored egui-wgpu
configures a new window's surface inside `winit::catch_errors` and returns a validation,
out-of-memory or internal error as `WgpuError::ConfigureSurface` (see `vendor/README.md`). Still a
panic, as upstream: an error in a later reconfigure (resizing, a lost surface); a device lost
during that first configure (wgpu reports it only to a callback, so the first frame panics, after
the app has started); and a window shown at zero size, which is first configured on resize. If
OpenGL can't start either, PdfCraft exits with both errors in the log. The fallback is logged at
`error`, with the wgpu error. `PDFCRAFT_RENDERER` skips it or chooses OpenGL from the start.

## Environment variables

| Variable | Effect |
|---|---|
| `RUST_LOG` | Log levels for standard error and the log file (see [Logs](#logs)) |
| `RUST_BACKTRACE` | `1` adds a backtrace to the report of an internal error |
| `XDG_DATA_HOME` | Linux/FreeBSD: base of the settings folder (`pdfcraft/`), the log folder and crash recovery |
| `WGPU_POWER_PREF` | GPU choice; by default PdfCraft draws on the GPU that drives the primary display (Windows) or the built-in panel (Linux), else the low-power (integrated) GPU. The log says which one was chosen |
| `WGPU_BACKEND` | Graphics backend; by default Windows uses Direct3D 12, falling back to OpenGL |
| `PDFCRAFT_RENDERER` | `gl` starts with OpenGL (glow) and never loads wgpu; `wgpu` reports a wgpu failure instead of retrying with OpenGL; unset, wgpu is retried with OpenGL when it can't start (see [Renderer fallback](#renderer-fallback)) |
| `CRAFT_FONTS_DIR` | Build time: a [craft-fonts](https://github.com/storytold/craft-fonts) checkout to embed (Japanese fonts) |
| `PDFCRAFT_SYSTEM_FONTS` | `0` disables both runtime UI fallbacks (`cargo xtask screenshots` sets it); no effect on fonts embedded in PDFs |
| `PDFCRAFT_TEST_REQUIRE_CJK` | `1` makes the Chinese interface-font test fail instead of skipping when the machine has no Chinese face (the macOS release job sets it) |

## Installed interface fonts

Chinese has no embedded face: the craft-fonts build input's only `Hans` face is Noto CJK, which
`AGENTS.md` §1.1 rules out. So the desktop app reads one Chinese face that is already installed on
the machine — the same face the page renderer substitutes for a non-embedded Chinese CID font, so
one search answers for both (`crates/fonts/src/system.rs`, `pdfcraft_fonts::han`). It tries
Microsoft YaHei / SimHei / SimSun / JhengHei on Windows, PingFang or Hiragino Sans GB on macOS and
WenQuanYi / AR PL on Linux and FreeBSD, and every candidate must map a whole set of Chinese
characters in one collection face before it is used. The face serves the interface in every
language, because a Chinese file name appears in an English interface too.

Neither runtime face is read in a web build, and if the machine has no Chinese face the interface
keeps working with replacement glyphs. `PDFCRAFT_SYSTEM_FONTS=0` turns both off, on every platform.

Both are read once, capped at 32 MiB each and 16 collection faces, and used only for drawing. No
system font file is copied, converted, embedded in a PDF, committed, downloaded or included in the
installer. Use fonts legally installed on the user's machine; Microsoft's
[font redistribution FAQ](https://learn.microsoft.com/en-us/typography/fonts/font-faq) permits
Windows applications to use system-wide Windows fonts for interface display, while generally
prohibiting redistribution without a separate licence. Other fonts retain their own terms.

Published screenshots must use `PDFCRAFT_SYSTEM_FONTS=0`, as required by `AGENTS.md` §1.4.
For regression validation with an installed Chinese font, set
`PDFCRAFT_TEST_REQUIRE_CJK=1` and run
`cargo test -p pdfcraft-ui-egui --test chinese_system_font -- --nocapture`.
Without the flag the test skips when no Chinese face is installed; with it, the same run **fails**,
which is what the macOS release job does before packaging — a build whose Chinese interface would
show boxes cannot be shipped as a green pipeline. The Windows release job runs the same test
*without* the flag: which fonts a CI image carries is nobody's promise, while every Windows desktop
install ships Microsoft YaHei, so a missing face there is the runner's problem and not a reason to
block a release. The test checks every non-ASCII
translated character in `zh-hans.tsv` and `zh-hant.tsv`, Chinese file names in both UI font orders,
all four interface families, and retention of the existing Arabic fallback. Run the same test in a
separate process with `PDFCRAFT_SYSTEM_FONTS=0` to verify the publication opt-out.
