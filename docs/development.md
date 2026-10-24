# Developing PdfCraft

Working instructions for agents and contributors are in `AGENTS.md` and `CLAUDE.md`. This page
collects what the desktop app reads from its environment and where it writes its diagnostics.

## Low-disk Cargo profile

When local build storage is constrained, select the opt-in `low-disk` profile explicitly:

```sh
cargo build --profile low-disk -p pdfcraft-cli
cargo test --profile low-disk -p pdfcraft-cos --lib
```

The profile inherits the normal development settings, including debug assertions, overflow
checks and line-table-only debug information, and disables incremental compilation for this
profile only. Ordinary `cargo build` and `cargo test` keep incremental compilation.

The development profile itself keeps only line tables for workspace code and no debug information
for dependencies. Backtraces show file and line numbers but no local variables or parameters; for
full debug information in workspace code (for a debugger), build with
`CARGO_PROFILE_DEV_DEBUG=true`.

Cargo stores custom-profile artifacts separately under `target/low-disk`. Switching between the
normal and low-disk profiles can therefore leave both sets of artifacts on disk. To remove only
the low-disk artifacts, run `cargo clean --profile low-disk`.

Building PdfCraft requires Rust 1.95 or later: the UI crate and its `egui` and `craft-ui`
dependencies need it, and the workspace `rust-version` states the same minimum.

## Interface appearance

Display theme saves System, Light or Dark separately from the palette currently drawn. New
installations still default to Light. System follows native appearance events on macOS and
Windows. The native window keeps its inherited appearance in every mode; both egui style branches
use the selected application palette, so desktop changes cannot override a manual choice.

On Linux the native app starts an XDG Settings portal subscription when System is first selected.
It reads `org.freedesktop.appearance` / `color-scheme`, where numeric `1` requests Dark, `2` requests
Light, and `0` means no preference. When that states no preference or is missing, it falls back to
`org.gnome.desktop.interface` / `color-scheme` (`prefer-dark`, `prefer-light`, `default`), which
XFCE's portal answers instead of the freedesktop namespace; KDE answers only the freedesktop one. The subscription is installed before the initial read. Initial
results and later `SettingChanged` signals wake an idle window. There is no periodic desktop
polling, and signals received in a manual mode update the cache without requesting frames.

System falls back to Light if no appearance is available, including an absent portal, an
unsupported appearance backend, no preference, an unknown value or a failed connection. This
does not rewrite the saved System choice. The watcher cancels when its window closes; it does not
reconnect after a session-bus disconnection. Headless UI contexts use their supplied egui input and
do not contact a desktop session bus. Page and document colours retain their existing behavior.


## Logs

The desktop app writes its `log` records to standard error and to `logs/pdfcraft.log` in its
data folder: Linux and FreeBSD `~/.local/share/pdfcraft/logs/` (or
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

For blurry or incorrectly scaled UI reports, attach `pdfcraft.log`. Look for the successful
startup `renderer:` line, which records the wgpu adapter, device type, backend and enabled texture
limit, or the OpenGL renderer. Also look for the `drawing it at … and scaling it up` warning: it
means PdfCraft hit the surface-size safety fallback and rendered below the physical window size, so
softness is expected.

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
`error`, with the wgpu error. Renderer resource creation now uses the same scopes and returns
`WgpuError::CreateRenderer` before the app is created, so a rejected `egui_pipeline` can also
reach this retry instead of the uncaptured-error panic handler. This does not fix the failing
driver's shader compilation, and explicit `PDFCRAFT_RENDERER=wgpu` still disables the retry.
`PDFCRAFT_RENDERER` skips it or chooses OpenGL from the start.

## Preferences

Linux and FreeBSD store `app.ron` in `$XDG_CONFIG_HOME/pdfcraft/`, or
`~/.config/pdfcraft/` when that variable is unset, empty or relative. This follows the
[XDG Base Directory Specification](https://specifications.freedesktop.org/basedir/latest/).
On first launch after upgrading, existing `app.ron` preferences are copied from the old
data folder without replacing an existing configuration file. The old copy is retained;
logs, crash recovery and digital IDs stay in their data locations. If migration fails,
the app logs the failure and continues using the original preferences folder.
The existing PrintCraft-to-PdfCraft migration runs first. macOS, Windows and portable
copies retain their current preferences locations.

## Environment variables

| Variable | Effect |
|---|---|
| `RUST_LOG` | Log levels for standard error and the log file (see [Logs](#logs)) |
| `RUST_BACKTRACE` | `1` adds a backtrace to the report of an internal error |
| `XDG_CONFIG_HOME` | Linux/FreeBSD: absolute configuration base for `pdfcraft/app.ron`; defaults to `~/.config` |
| `XDG_DATA_HOME` | Linux/FreeBSD: base of the log folder and crash recovery; previous preferences are migrated from here |
| `WGPU_POWER_PREF` | GPU choice; by default PdfCraft draws on the GPU the Wayland compositor draws with (it says so through linux-dmabuf feedback), else the GPU that drives the primary display (Windows) or the built-in panel (Linux), else the low-power (integrated) GPU. The log says which one was chosen |
| `WGPU_BACKEND` | Graphics backend; by default Windows uses Direct3D 12, falling back to OpenGL |
| `WGPU_DX12_PRESENTATION_SYSTEM` | Windows, Direct3D 12: `hwnd` presents through a swap chain on the window itself, which Windows stretches while the window is resized (#2738); unset or `visual`, PdfCraft presents through a DirectComposition visual |
| `WGPU_VALIDATION_INDIRECT_CALL` | `1` turns wgpu's check of indirect draw arguments back on. PdfCraft draws nothing indirectly and turns it off, since the compute shader the check needs fails to compile on some drivers (Metal on an Intel Iris Pro), which lost the device at start-up |
| `PDFCRAFT_RENDERER` | `gl` starts with OpenGL (glow) and never loads wgpu; `wgpu` reports a wgpu failure instead of retrying with OpenGL; unset, wgpu is retried with OpenGL when it can't start (see [Renderer fallback](#renderer-fallback)) |
| `PDFCRAFT_TITLEBAR` | Linux: `app` draws PdfCraft's own title bar (window buttons in the tab strip, in the order of GNOME's `button-layout`, and resize edges); `system` keeps the frame winit or the desktop draws; unset, PdfCraft draws its own when the Wayland compositor draws no frames (GNOME), and leaves it to the desktop otherwise (KDE, Sway, Hyprland, X11). Where PdfCraft draws it, Preferences ▸ Use system title bar switches back to winit's frame without a restart; `system` also hides that option |
| `PDFCRAFT_BUILD_LABEL` | Build time: a label for a local or patched build (letters, digits, inner dots), shown after the version in `--version`, About and the log (`0.7.1 (titlebar)`). `packaging/linux/package.sh` also puts it in the file names and the .rpm/.deb release (`1.titlebar`), which upgrades over the release of the same version |
| `CRAFT_FONTS_DIR` | Build time: a [craft-fonts](https://github.com/storytold/craft-fonts) checkout to embed (Japanese fonts) |
| `PDFCRAFT_SYSTEM_FONTS` | `0` stops the desktop app from using an installed font for characters its embedded fonts lack (`cargo xtask screenshots` sets it) |
| `FONTCONFIG_FILE` | On Linux/Unix, the Fontconfig configuration file used to discover installed UI fallback fonts (default: `/etc/fonts/fonts.conf`); relative filenames are resolved under `FONTCONFIG_PATH` |
| `FONTCONFIG_PATH` | Directory for a relative `FONTCONFIG_FILE` (default: `/etc/fonts`; the first entry of a search path is used) |

On Linux/Unix, the UI's existing fallback filenames are also searched in Fontconfig's configured
directories and their subdirectories. This finds fonts in Nix store paths and user font directories
without invoking `fc-match` or linking libfontconfig. The scan is cached and bounded; missing or
malformed configuration leaves the ordinary `/usr/share/fonts` and `/usr/local/share/fonts`
candidates available. Font size and glyph-coverage checks still apply. This only reads installed
fonts for the interface; it does not embed them into PDFs or packages.

Relative includes resolve beside the config that contains them. Included directories load only
digit-prefixed `*.conf` files, in lexical order; backups and disabled files are ignored. The scan
limits each config to 1 MiB, visits at most 256 config files/directories, and bounds include depth
to 16 and directory entries to 32,768. Cyclic includes are skipped.
