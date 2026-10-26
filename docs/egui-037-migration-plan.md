# Current integration — 2026-10-25

The local integration includes main `d764a8ebca2dc289841b34ea3048f613a9869bbb` and pins craft-ui, craft-quit and craft-print to published `42e30cd5b3f4b114270dd053fccbc33bedc47f68`. The egui, winit and independent craft-docpath pins are unchanged. Its `Cargo.lock` SHA-256 is `3e0c10576c8584eb1f45a27908177e7c65bb83f8e855bf3a5429edd4e7268c80`.

Main replaced the floating docking workspace with fixed shared side panes in `crates/ui-egui/src/side_panels.rs`. `PaneHost` owns the Tools and inspector columns; `Rail` owns the fixed navigation rail and footer. The integration retains the 0.37 ID, text alignment, capture, IME and accessibility adaptations. The former `docking.rs`, `rail-nav` panel and read-mode inspector panel no longer exist. Floating windows, tab groups and drag/redock operations from the earlier design are not capabilities of this integrated UI. Their historical acceptance items below do not establish current behavior.

Focused scroll, zoom and RTL palette regressions passed. Removing the production fixes while retaining the tests reproduced double scrolling, double zooming and failed reordered-word RTL search; both source files were then restored byte-for-byte. Independent source review accepted the integration. Full combined-tree CI and the separate web WASM check passed with required external oracles: 5,190 passing test executions, zero failures and 22 ignored across 89 summaries. The first attempt stopped at the storage reservation guard; the successful rerun used an increased reservation. The 5,190 result comes from managed run `egui037-pdf-combined-ci-20261025-1340` and qualifies the integration of main `d764a8ebca2dc289841b34ea3048f613a9869bbb` with `Cargo.lock` SHA-256 `3e0c10576c8584eb1f45a27908177e7c65bb83f8e855bf3a5429edd4e7268c80`, published as `fbfad2039c650a02b39ceee5c3f0e4d6fd4b3f74`. The older 5,183-pass result covered commit `9032aeb35f7abe7777f66d4b835fd461a865c516`, integrating main `71b47b51`; it is a separate checkpoint and is not evidence for the current tree. Both runs recorded 22 ignored executions across 89 summaries.

Remaining acceptance includes native/browser GUI, native IME, packaging and paired performance. Shared-pane F6 cycling and explicit RTL edge mirroring are not enabled. New side-pane command labels still need catalog translations; English fallback remains. Current panel checks must exercise Tools/inspector visibility, Read mode, resizing and persistence, rail selection, F4/Shift+F4, capture and accessible Pane/Splitter labels, rather than the removed floating/tab-group workflows.

The qualification tables and original porting protocol below are historical records. Their old pins, lock hashes and docking design describe those checkpoints only; this section supersedes them for the current integration.

> Historical records begin below this notice. Their source identities, lockfile hashes and test counts describe earlier checkpoints only; they do not qualify the current `d764a8eb` integration. The current integration and its 5,190-pass qualification are recorded above.

# PdfCraft — egui 0.37 porting plan

Status update (2026-10-25): Move PdfCraft to the pinned egui 0.37 and winit forks, preserving main `418ac6a2`, including suppression of page link hover while dragging a panel, page-pointer ownership, editor gestures and navigation/security fixes. Adapt UI lifecycle, capture and IME reset handling while preserving logical RTL text. Restore document-properties filename alignment and connect Initial View controls to their accessible labels. Validation: all ten CI stages and the separate web-shell WASM check passed. Focused pointer regressions plus CI recorded 5,130 passing test executions, zero failures and 28 ignored across 143 summaries. The dragged-panel link regression passes on egui 0.37; independent review accepted preservation of the upstream fix. The alignment and accessible-name regressions failed before their fixes and pass afterward; independent reviews accepted the integration and both fixes. Pins and lockfile remain unchanged. Native/browser GUI, native IME, paired app performance and final platform/package acceptance remain open. Human review and human merge are required.

The original planning record below is historical.

Status update (2026-10-25): Move PdfCraft to the pinned egui 0.37 and winit forks, preserving main `24edde65`, including page-pointer ownership, editor gestures and navigation/security fixes. Adapt UI lifecycle, capture and IME reset handling while preserving logical RTL text. Restore document-properties filename alignment and connect Initial View controls to their accessible labels. Validation: all ten CI stages and the separate web-shell WASM check passed. Focused dialog regressions plus CI recorded 5,111 passing test executions, zero failures and 22 ignored across 143 summaries. The alignment and accessible-name regressions failed before their fixes and pass afterward; independent reviews accepted the integration and both fixes. Pins and lockfile remain unchanged. Native/browser GUI, native IME, paired app performance and final platform/package acceptance remain open. Human review and human merge are required.

The original planning record below is historical.

Status: implemented locally against `a2ec27b4775f53c40cad3456936d9406611e3a8d`; not published. Production code, manifests, vendor provenance and tests in this checkout carry the migration. The 2026-10-25 qualification section below supersedes the historical external-config and missing-IME statements; the 2026-10-24 amendment records the earlier local state. The manifests pin egui `6d518fc799681571963842f95b55a6b68431b15a`, winit `a517b3846ee8457d84ee4975110bfda6797c44dc`, and craft-ui/craft-quit/craft-print `25da1314d443171b4adeaad2cd9454b273f35c6f` directly by immutable git revision, while `craft-docpath` remains at `82a24b006eefa036b43c09a67fb9a3fbd17ffe63`; `Cargo.lock` is `f5cee0acb795240d2071116230316fd4e4b6bfe4e4314c82b0a4ca5459739f80`, and the target-main observation is `a7801dc84e74dd12757aecbb6be324dce7a33939`. The repository quality gates passed as recorded below; publication still requires fresh human approval. This revision also incorporates the independent adversarial review of the prior plan (`f781bf52d03d59cd2fb4539771c7a0d196e2f49e64c48635e72c224c9bdcdcd0`).

## Historical qualification state — 2026-10-25

The managed run `egui037-pdfcraft-reviewed-ui-ci-20261025-0541` passed the first nine CI stages, including formatting, strict lint, workspace tests, WASM compilation, assets and dependency checks. It recorded 5,025 passing test executions, zero failures and 16 ignored across 198 summaries (including four focused regression executions). The final parity stage found one stale test reference after the Arabic logical-order test was renamed; the reference was corrected without changing executable source. Formatting and parity then passed in `egui037-pdfcraft-parity-final-20261025-0555`. An intervening submission `0553` was refused by the storage guard before execution and is not validation evidence. Final documentation/attribution corrections were checked by formatting, assets generation/validation and parity in `egui037-pdfcraft-final-doc-assets-20261025-0571`.

The page-text editor keeps the prior four-by-two inset under egui 0.37, and print tests locate the Two-sided control by ComboBox role. Both narrow/wide text image regressions and booklet regressions passed before the full suite. Canonical dependency/source guards retain the vendored security and rendering provenance. The repository WASM gate checks the library layers. The separate web application shell also passed `cargo check --locked --target wasm32-unknown-unknown -p pdfcraft-web` in `egui037-pdfcraft-web-shell-check-20261025-0575`. Native GUI and IME/PRIMARY, browser interaction, packaged WASM size, real-world old settings and paired app performance remain open; these checks do not establish cross-platform acceptance.

This section supersedes every historical statement below that the egui, winit or
craft-libs revisions resolve through `local-integration-config.toml`, that the
checked-in manifest does not pin them, or that E21b/P7b surrounding-text support is
absent from the candidate. Those statements are planning-era history; the current
tree does not use an external Cargo config.

| Item | Current value |
|---|---|
| egui family | `6d518fc799681571963842f95b55a6b68431b15a` (storytold/egui) |
| winit | `a517b3846ee8457d84ee4975110bfda6797c44dc` (storytold/winit) |
| craft-ui / craft-quit / craft-print | `25da1314d443171b4adeaad2cd9454b273f35c6f` (storytold/craft-libs) |
| `Cargo.lock` SHA-256 | `f5cee0acb795240d2071116230316fd4e4b6bfe4e4314c82b0a4ca5459739f80` |
| Target-main observation | `a7801dc84e74dd12757aecbb6be324dce7a33939` |

`Cargo.toml` pins those revisions directly and `Cargo.lock` resolves them to the
same immutable git sources; no `local-integration-config.toml` is read. The
retained `vendor/winit` and `vendor/egui-wgpu` copies remain provenance for the
patch text and source guards, not runtime path patches.

The pinned egui `6d518fc` tree contains the `TextEdit` outgoing
surrounding-text/`IMEOutput` producer and egui-winit calls
`set_ime_surrounding_text`. PdfCraft adds no custom adapter. Native platform
execution and the outgoing-text tests remain unqualified.

## 1. Baseline and scope

| Item | Value |
|---|---|
| PdfCraft source | current owned checkout `a2ec27b4775f53c40cad3456936d9406611e3a8d`; the original planning baseline was `0dc77fe4ed8810b9e9f25b9dd050afae662c062a` (`docs/egui-037-plan-20261022`, tree `ebb7259286988cf937ade02ab08db67064b04084`) |
| PdfCraft lockfile | current `Cargo.lock` is the direct-pin qualification lock (SHA-256 `f5cee0acb795240d2071116230316fd4e4b6bfe4e4314c82b0a4ca5459739f80`); it is not publication evidence. The planning baseline lock SHA-256 was `eaee6973f7c1ba79f5bfc1bfa3da50ef457d13affeb51a3d50d27437804d4cf7`. |
| Planning-baseline egui family | `egui`, `eframe`, `egui-winit`, `egui_extras`, `egui_kittest` 0.36.2 from crates.io; `egui-wgpu` 0.36.2 vendored |
| Planning-baseline winit | 0.30.13 vendored; upstream tag commit `e9809ef54b18499bb4f2cac945719ecc2a61061b` |
| Planning-baseline craft-libs pins | `craft-ui` `5c874d51eeb3de70e735c8ce87a59a7dedd47675`, `craft-quit` `f58bc1a3df490992d633393d0898af0588ce882c`, `craft-print` `666c358065261a2816b376fa3bb10538dbf53040`, `craft-docpath` `82a24b006eefa036b43c09a67fb9a3fbd17ffe63` |
| Planning-era shared-libs integration base | `3832c47ef68a0e6214f2c1e85f3526b6adcb686e`; an ancestor of the current craft-libs pin `25da1314d443171b4adeaad2cd9454b273f35c6f` |
| Historical planning observation of craft-libs PR 199 | OPEN / CONFLICTING at the then-inspected head `b68de8c5f4f1c060904d211a7b1320aa0f7bce78`; API-port ancestor `8614e4a04fae516fba16c22b31ff1008cb6be10b`; audit merge `ec8840af8c5fda695cf195cdd05e5ae078415c07` |
| Target egui | 0.37.0, upstream release commit `841a328ba2dd8f6996beb7189ebb7fc00e668a2a` |
| Planning-era reviewed egui candidate | `aiongg/craft/0.37` HEAD `a4f9a17f64a6bae61350fb85804c9df0d9865916`; the current pin is `6d518fc799681571963842f95b55a6b68431b15a` |
| Planning-era reviewed winit candidate | `craft/0.30.13-lang` HEAD `8b35388bd719e8cc86a95971ba0bc026c9a8f058`; it lacks W15P PRIMARY, which the current pin `a517b3846ee8457d84ee4975110bfda6797c44dc` contains |
| Shared rollout contract | `craft-libs/docs/egui-037-suite-rollout.md`, current reviewed content identified by the publication/review ledger; record its landed commit before implementation |
| Shared library plan | `craft-libs/docs/egui-037-migration-plan.md`, current reviewed content identified by the publication/review ledger; record its landed commit before implementation |
| Shared egui plan | `egui/docs/egui-037-migration-plan.md`, current reviewed content identified by the publication/review ledger; record its landed commit before implementation |

The following paragraph records the pre-merge planning assessment; PR 199 subsequently merged, and the current pinned shared-library candidate includes its integration and later main work. At planning time, PR 199 was an input to the shared-library work, not a branch to merge. Its head and `ec8840a` are stale relative to current main: they omit `craft-platform` and revert security, settings, docking and printing fixes. Only the API-port hunks from `8614e4a` and the README change from `1764bca` are reused, applied onto `3832c47`; the final library revision must be reviewed and published separately.

This plan covers only PdfCraft. It does not adopt, reset or migrate any other checkout, job, VM or shared target. Historical diagnostics retained in the private task evidence for `egui037-audit-20261022` are pointers only; they are not acceptance evidence.

Entry points:

- Native: `apps/pdfcraft/src/main.rs::main` calls `eframe::run_native`; `app_creator` (`main.rs:365`) builds `PdfCraftApp` and wires storage, control, recovery and GPU state.
- Web: `apps/pdfcraft-web/src/main.rs::main` uses `eframe::WebRunner::start` with `WebOptions { renderer: Glow }`, `?file=`, `?host=parent` and the before-unload warning.
- Headless: `crates/ui-egui/examples/shot.rs` uses `egui_kittest::Harness::build_eframe` plus `Harness::render`; `apps/pdfcraft-cli` has no egui/eframe dependency.
- Tests: `crates/ui-egui/tests/it/main.rs` links the integrated UI tests; `apps/pdfcraft/tests/it/main.rs` links twelve winit source guards; `crates/ui-egui/src/control.rs` has the in-crate control/pointer probes; `crates/ui-egui/tests/perf.rs` is the existing timing harness.

Targets to keep green:

- `apps/pdfcraft` (native binary; wgpu plus glow fallback, accesskit, persistence, links, x11/wayland, native clipboard).
- `apps/pdfcraft-web` (wasm32, eframe glow; no egui-winit clipboard feature is needed or available there).
- `apps/pdfcraft-cli` (no egui dependency; unchanged).
- `crates/ui-egui` (library, unit tests, `tests/it` integrated binary, `tests/perf.rs`, `examples/shot.rs`).
- `xtask` gates (`fmt`, clippy, test, layers, wasm, assets, deny, parity).

## 2. Actual lifecycle, features and compatibility surface

### 2.1 App boundary

`PdfCraftApp` is declared in `crates/ui-egui/src/lib.rs`; the eframe implementation is `impl eframe::App for PdfCraftApp`:

- `raw_input_hook` applies line-scroll speed and injects the OS drag pointer position before egui reads the frame.
- `save` repairs minimized-window storage through `window_state::WindowState::keep`.
- `logic` owns host-side work: context capture, window repair, i18n selection, font installation, UI scale, drag/drop completion, inbox/open queues, OS events, control-channel requests, quit guard, autosave, file watching, update/print/export/OCR/optimize/action polls, deferred shortcuts, per-view input blocking, pending edits and picker completion.
- `ui` owns per-pass layout: title command, full-screen or normal panels, tab strip, mode bar, right rail, docking or read-mode central panel, palette, dialogs, progress, toasts, then `finish_render_frame`.
- `finish_render_frame` suspends hidden tabs' render pools and commits the visible/compare views' frame demand.

`crates/ui-egui/src/canvas.rs` is a second lifecycle: `DocView::begin_render_frame` resets demand and the frame queue; `DocView::finish_render_frame` builds the render queue, feeds the pool and calls `DocView::receive` for a bounded upload batch.

### 2.2 Lifecycle truth: `logic` is not a frame clock

At the pinned 0.37 source, both native eframe (`crates/eframe/src/native/epi_integration.rs::update`) and web (`crates/eframe/src/web/app_runner.rs::logic`) call `app.logic` and then `app.ui` inside the `Context::run_ui` closure. A discarded layout reruns both. Hidden or occluded windows can call `App::logic` through `Context::run_logic`, which updates only raw viewport/window-focus information; events, input time, layout and normal pass state remain from the previous UI pass. Input therefore belongs only in a known UI phase (`App::ui` or an explicit method it calls), never in background work that reads `ctx.input` under the assumption that the window is hidden.

Two corrections are binding:

- `ViewportInfo::visible()` describes minimized/occluded state; it does not prove that the current callback is logic-only. A hidden UI paint can be requested for screenshots or injected input.
- After a normally completed `run_ui`, `current_pass_index()` is zero even after multiple passes: `ContextImpl::end_pass` takes the viewport output with `mem::take`, and `run_dyn` restores the count only immediately before the next UI pass. A first-pass guard does not by itself skip ordinary hidden `run_logic` calls. It is still not a frame clock and cannot supply fresh input.

No `run_ui_with_logic`/E22 scheduler API is adopted. There is no stable one-frame boundary inside `run_ui`, and both hosts already run logic per pass. Queues keyed by consumed identity, command/generation identity and monotonic deadlines provide the required behavior.

### 2.3 Hidden work and current input

- `App::logic` may run repeatedly in one visible frame and may run alone while hidden. Autosave, recovery, playback/audio, open completion, file watching, polls and undo application stay there, but use monotonic elapsed time (`std::time::Instant`, `web_time::Instant` on wasm), not `ctx.input(|i| i.time)`.
- Fresh wheel, pointer, keyboard and IME input is handled in `App::ui` before panels. `Motion::begin_pass` moves from `logic` into `ui` (before any widget reads wheel input) or is guarded by a fresh-UI-pass token; it must never consume the same `ctx.input` event twice.
- The grid-drop deadline and control-adjacent timers use monotonic deadlines too.
- UI-produced commands may commit promptly from the UI. The requirement is exactly-once consumption, not "never commit on a discarded pass": egui cannot know at commit time whether a later widget will request a discard. Screenshot completion and render queues may be cancelled without side effects; an edit/undo transaction may not be duplicated when the pass is replayed.

### 2.4 Feature and cfg inventory

| cfg / feature | Where it matters |
|---|---|
| `target_arch = "wasm32"` | `apps/pdfcraft-web`; `ui-egui` disables `file_watch`, `pickers`, `system_fonts`; `fill_sign` image inbox and host bridge; no native window and no egui-winit clipboard |
| `windows` | hidden-maximized restore, DPI change, DWM frame, IME cursor, auto language, measurement units, DX12 presentation (`apps/pdfcraft/src/main.rs`, `motion.rs`, `single_instance.rs`, winit patches W1/W3/W4/W18/W19) |
| `target_os = "macos"` | Apple events, traffic lights/full-size content view, ⌘Q menu (`craft-quit`), title-bar drag, IME commit/discard/emoji/Korean (W6–W10, W12, W22–W24, W26) |
| `target_os = "linux"` | GNOME/Wayland frame decision, compositor GPU selection, Wayland regular clipboard and W15P PRIMARY (W15/W15P/E4), X11 DPI/IME (W5/W11/W20/W21), Wayland scroll phases (W13) |
| `unix, not macos` | single-instance Unix socket, Wayland/X11 detection, fontconfig directory parsing |
| `not unix` / other | Windows and wasm branches above; no silent fallback to a native-only API |
| Native eframe features | `default_fonts`, `wgpu`, `glow`, `accesskit`, `persistence`, `links`, `wayland`, `x11`, **`clipboard`** |
| `ui-egui` eframe features | `default_fonts`, `wgpu`, `accesskit`, `persistence`, `links`, **`clipboard`** for native consumer/test parity |
| Web eframe features | `default_fonts`, `glow`, `persistence`, `accesskit`; no winit clipboard path |
| egui features | `bytemuck` in `ui-egui`; `default_fonts` explicit; **`monochrome_emoji_fonts`** in every egui-consuming target that draws PdfCraft UI text |
| eframe `system_fonts` | stays off; `NativeOptions::system_font_fallback = false` |

`cargo xtask wasm` checks crates below L8; `cargo check -p pdfcraft-web --target wasm32-unknown-unknown` remains required for the actual web shell. A CI feature-identity assertion must show `egui-winit/clipboard` enabled for native roots after the migration, and `cargo deny`/`cargo metadata` must show exactly the approved fork sources.

### 2.5 Control channel and screenshots

`crates/ui-egui/src/control.rs` is the user-observable automation surface. Today it installs `ControlPlugin`, stores the AccessKit tree and injects queued input in hooks, handles requests in `Control::tick`, receives screenshots as `egui::Event::Screenshot`, sends `ViewportCommand::Screenshot(UserData::new(tag))`, crops and PNG-encodes in `screenshot_png`, and times out after 5 seconds. Its tests live both in `src/control.rs` cfg-test modules and `crates/ui-egui/tests/it/control.rs` (65 tests, linked by `tests/it/main.rs`), including discard handling and `screenshots_of_window_and_region`. PR 1 updates both sets.

egui 0.37 removes `Event::Screenshot` and changes `ViewportCommand::Screenshot` to a one-shot `ScreenshotCallback`. A pending screenshot command itself asks the integration to paint a hidden window; `RequestPaintWhileHidden` is additionally required when injected input must be applied without a screenshot.

`Shared::passes` is a pass counter, not a frame counter. The migration renames the unit to accepted frames and increments it in `output_hook` only when `output.platform_output.request_discard_reasons.is_empty()`; an intermediate discarded pass does not count. A request made before a forced discard is not answered before the intended number of accepted frames.

### 2.6 Panels, docking and window chrome (planning baseline)

PdfCraft uses the 0.37 `Panel` API already in its 0.36 form:

- `chrome::tab_strip`: `Panel::top("tab_strip")`;
- `chrome::mode_bar`: `Panel::top("mode_bar")`;
- `chrome::right_rail`: `Panel::right("rail")` and `Panel::bottom("rail-nav")`;
- read-mode inspector: `Panel::right("read_right_panel")`;
- central panel in `lib.rs`.

0.36 stores `PanelState` under the panel `Id` passed to the constructor. 0.37 derives it as `parent_ui.scope_id().with_salt(IdSalt::new(id_salt))`; the panel child `Ui` is also salted with the new panel ID. Every named panel key therefore changes, and in-panel widget/focus/scroll state may reset. This is not left to implementation: §6.2 migrates the persisted keys in the first compatibility PR and tests the upgrade from a 0.36 fixture.

Docking is `crates/ui-egui/src/docking.rs`; `DockArea::new(egui::Id::unique("pdfcraft-docking"))` wraps `craft_ui::docking`. Persistence is PdfCraft's own `Workspace` JSON, not egui panel memory. `DockArea::direction` is a new opt-in stock-compatible API planned in craft-libs CL-3; the explicit RTL adoption in PR 4 waits for that reviewed revision. Its visual policy preserves saved physical splits, rectangles and logical tab order.

### 2.7 Window frame, decorations and restoration

`apps/pdfcraft/src/main.rs` owns native options, app creation and GPU setup. `apps/pdfcraft/src/decorations.rs` decides whether GNOME Wayland leaves the frame to the app. `crates/ui-egui/src/window_controls.rs` draws the app frame and resize edges, and `window_state.rs` repairs a 64×64 restored window.

`craft-window` owns `Frame`, `Layout` and `Button`; `window_controls.rs` currently has distinct duplicate types and its own GNOME `button-layout` parser. The migration makes `window_controls` re-export `craft_window::{Button, Layout}` and removes the duplicate parser. `decorations::app_drawn` uses `craft_window::Choice::probe(Prefix::new("PDFCRAFT"))`; `PDFCRAFT_TITLEBAR` accepts `app`, `system` and `desktop` (the last two force the desktop frame), matching craft-window. PdfCraft still applies `with_decorations(false)` only when it will draw its own header bar, and the runtime “Use system title bar” preference still toggles `ViewportCommand::Decorations`; craft-window decides the startup frame, not the runtime preference. The probe is called once at startup and never from `App::ui`.

### 2.8 Fonts and language

`theme::install_fonts_for` calls `ctx.set_fonts(installed_font_definitions(cjk))`; `installed_font_definitions` appends one installed face plus optional script fallbacks. `theme::on_interface_baseline` computes `FontTweak::y_offset_factor` from primary and fallback metrics. `system_fonts::fallback` is the single-face, last-resort policy; `PDFCRAFT_SYSTEM_FONTS=0` disables it.

At the planning baseline, the i18n layer preordered right-to-left translations at catalog load (`i18n::LangInfo.rtl`, `catalog.map_translations(display_rtl)`), and `bidi::display_rtl`, `bidi::visual`, `bidi::wrap` and `bidi::visual_auto` preordered runs because egui 0.36 does not. The current migration removed catalog preordering; `display_rtl` and the other preordering helpers remain test-only. The fork's E5/E12/E13/E15/E16/E17/E20 own reordering now. Keeping `display_rtl` in production alongside E5 would double-reorder text.

### 2.9 Clipboard, IME and input

Text copy/paste goes through egui events; images and file paste use `arboard` directly. Native eframe 0.37 no longer enables `egui-winit/clipboard` implicitly when `default-features = false`; without the explicit feature, text cut/copy/paste loses OS integration and the Wayland clipboard/PRIMARY path is compiled out. `arboard` image/file code can mask that loss in a superficial check.

PdfCraft uses ordinary `egui::TextEdit` for its dialogs, forms, comments composer and text editor; it has no custom IME editor producer requiring an app-side surrounding-text adapter. The pinned egui `6d518fc` contains the TextEdit producer/`IMEOutput` surrounding-text path, and egui-winit calls `set_ime_surrounding_text`; native platform execution and outgoing-text tests remain unqualified.

## 3. Dependency, lockfile and vendor audit

### 3.1 Manifests

| Manifest | egui/eframe facts and required change |
|---|---|
| `Cargo.toml` workspace | egui/eframe/egui_extras/egui_kittest 0.36; `[patch.crates-io]` contains `winit = vendor/winit` and `egui-wgpu = vendor/egui-wgpu`. Replace window-stack path patches with approved immutable fork revisions; retain PDF vendors |
| `crates/ui-egui/Cargo.toml` | eframe features `default_fonts`, `wgpu`, `accesskit`, `persistence`, `links`; add `clipboard`; egui features gain `monochrome_emoji_fonts`; egui_kittest dev-dep `eframe`, `wgpu` |
| `apps/pdfcraft/Cargo.toml` | eframe features `default_fonts`, `wgpu`, `glow`, `accesskit`, `wayland`, `x11`, `persistence`, `links`; add `clipboard`; Linux dev-dep winit 0.30 for the scroll-policy test |
| `apps/pdfcraft-web/Cargo.toml` | eframe features `default_fonts`, `glow`, `persistence`, `accesskit`; no clipboard feature or egui-winit dependency |
| `apps/pdfcraft-cli/Cargo.toml` | no egui/eframe; unaffected |

egui 0.37 no longer enables `default_fonts` by default. PdfCraft explicitly enables it in its own features. `monochrome_emoji_fonts` is chosen (not replacing `✔`/`▲`/`▼` with new SVG glyphs): the fonts are already OFL/MIT, attributed, and preserve existing text symbols deterministically with `PDFCRAFT_SYSTEM_FONTS=0`. The painted submenu arrow is not a font glyph after 0.37 and needs no emoji font.

### 3.2 Lockfile duplicates and family resolution

At planning time the lockfile contained one egui family at 0.36.2; the current qualification lockfile resolves the family at 0.37.0 (see `Current qualification state — 2026-10-25`). Several `vello_common`/`vello_cpu` minors still come through the PDF renderer and epaint. The target state must prove one egui family, one epaint family and one winit:

- `egui`, `epaint`, `emath`, `ecolor`, `eframe`, `egui-winit`, `egui-wgpu`, `egui_extras`, `egui_kittest` all at 0.37.0 from one source.
- `winit` at 0.30.13 with the craft language patches, from one source.
- `wgpu` remains 30.x; `egui-wgpu` matches the resolved wgpu.
- `accesskit` moves 0.24.x → 0.25.x; `accesskit_winit` 0.32.x → 0.34.x; epaint moves to `vello_cpu` 0.2.0 while hayro remains on 0.0.8. E11 is not required for PdfCraft because these versions cannot unify (verify with `cargo tree -e features -i vello_cpu`).
- Verify `cargo metadata` package versions and sources, not tag names.

### 3.3 Vendored winit provenance and compatibility

`vendor/winit` is 0.30.13 (`vendor/winit/.cargo_vcs_info.json` = `e9809ef5…`) and carries **W1–W13**, not W1–W15. It has no language APIs. The planning-era candidate winit fork `8b35388b` carried W1–W15 plus W17–W27 but not W15P; W16 is not in this cut. W14 (`c488d2e9`, macOS cancelable termination plus `dpi` from crates.io) and W15 (`cf73a29c`, `b05ce129`, per-seat Wayland data device and regular clipboard) are in the fork, not the current vendor.

W14 remains opt-in in the shared winit revision and is **not enabled** by PdfCraft; its existing termination path stays `craft-quit`. Validate that `dpi` resolves once with the git source. W15 regular clipboard is required by E4. In the planning-era candidate `8b35388b`, the **W15P** PRIMARY implementation was a prerequisite added by the shared winit work, using SCTK 0.19.2's `PrimarySelectionManagerState`/device/source/offer on the existing connection and event queue, and had to land before egui P2b. The current pin `a517b384` contains W15P (`PATCHES.md` W15P row; `src/platform_impl/linux/wayland/clipboard.rs`) and W27; native evidence remains open. No second regular `wl_data_device` is acceptable.

The egui fork requires companion winit APIs on their target platforms. Integrate and review the series in the approved public storytold fork, compare every current PdfCraft vendor delta, transfer behavioral tests, then replace the path patch with the final immutable git revision. Record provenance and old/new source hashes in migration evidence. Keep the original vendor until equivalence is established; do not publish an interim personal-fork branch dependency.

### 3.4 Source-guard migration and exact inventory

`apps/pdfcraft/tests/it/main.rs` links twelve winit guard modules. All twelve must be transferred to the winit fork (or replaced by equivalent fork-owned immutable tests), and only behavioral/ABI tests remain in PdfCraft:

`winit_macos_emoji_picker`, `winit_macos_ime_commit`, `winit_macos_ime_discard`, `winit_macos_korean_first_key`, `winit_macos_korean_space`, `winit_macos_title_bar_drag` (W12), `winit_wayland_scroll` (W13), `winit_wayland_text_input` (W2), `winit_windows_hidden_maximized` (W4), `winit_windows_ime_cursor` (W3), `winit_x11_dpi` (W11), `winit_x11_ime_focus` (W5).

PdfCraft keeps the retained `vendor/winit` and `vendor/egui-wgpu` copies as provenance and may read them only for provenance assertions (W1 patch text, base hashes, the W1–W13/E2+E3 rows), never as proof of the code the app runs. The resolved graph is checked from `Cargo.lock`: `vendor_graph.rs` requires the exact canonical storytold git URL, 40-character revision and fragment for the two crates it inspects, `winit` and `egui-wgpu`, and rejects path sources. W17 covers W2, W3, W5–W10, W12 and W13; add W1, W4, W11 and the egui-wgpu behavioral coverage to the same central evidence map. The W15/W15P guard belongs on the winit fork or a behavior test, never on `vendor/winit`.

### 3.5 Vendored egui-wgpu provenance

`vendor/egui-wgpu` is 0.36.2 (`49682f8b…`) with E2+E3. The target is 0.37.0 plus the same two patches. The reviewed fork's E2+E3 commit is `9dd799ab` and adds a `surface_fit` unit test; PdfCraft also has stronger guards:

- `apps/pdfcraft/src/main.rs::a_window_larger_than_the_gpu_limit_gets_a_surface_within_it`
- `apps/pdfcraft/src/main.rs::surface_fit_never_empties_or_overflows_a_surface`
- `apps/pdfcraft/src/main.rs::vendor_provenance_egui_wgpu_surface_fit_identity`
- `apps/pdfcraft/src/main.rs::a_gpu_error_while_a_window_is_set_up_is_returned_not_a_panic`
- `apps/pdfcraft/src/main.rs::vendor_provenance_egui_wgpu_catch_errors_identity`
- `apps/pdfcraft/src/main.rs::pipeline_creation_error_is_returned_and_device_remains_usable`

Integrate E2+E3 and the behavioral regressions into the central 0.37 fork, then consume the approved revision. Replace the source-path/source-absence assertions with immutable package identity checks. `surface_texture_limits` and `configure_gpu` remain: the first raises the enabled 2D limit, the second preserves adapter selection, DX12 presentation and indirect-validation behavior.

### 3.6 craft-libs: port only, and account for craft-print/GDI/rgb drift

PR 199's egui port changes `craft-ui` roles, `Id::unique`/`Ui::scope_id`, `egui_extras`/`egui_kittest` 0.37, and the docking fixture's `⋮` → `…`. Reuse those API hunks on current main; do not import the stale tree. PdfCraft needs `craft-ui` for docking/icons/tabs/layout, `craft-window` for the frame decision, and no wholesale move of its PDF-specific print UI into `craft_ui::printing`.

Bumping all four craft-libs crates from their four current pins listed in section 1 to a revision based on `3832c47` carries more than a new field:

- `craft-print` adds the unconditional `craft-winprint-gdi` dependency (`storytold/winprint.rs`, revision `417ac3c4…`), a public `rgb` re-export of the GDI transport, `lpstat -l` description parsing and `Printer.description`.
- Audit `craft-winprint-gdi` licensing/target cfg/`cargo deny`/Windows build, and the non-Windows `rgb::PrintError::Unsupported` behavior.
- `Printer` now has `description`; update every literal in the repository. At baseline there are 19 `Printer { … }` literals across `crates/print` and UI tests, not 13. This is a mechanical compile fix, with the craft-print tests and Windows native checks re-run.
- Keep PdfCraft's own `print_ui.rs` sheet preview and options. `description` is not automatically displayed; a product change to show it is separate.
- The current main already includes intervening security bounds, settings merge, docking permissions and linear printer-name collision checks; preserve them. The shared-library plan's CL-1 diff gate remains mandatory.

### 3.7 Publication and duplicate-resolution rules

- Pin immutable revisions. A branch name is not a pin.
- `[patch.crates-io]` in the egui or winit repository does not propagate into PdfCraft. PdfCraft's root must patch every resolved egui-workspace package, including `egui_glow` for GL, `epaint_default_fonts`, the used core/render/test crates, and winit.
- Use approved public immutable storytold fork revisions after vendor equivalence review. Never add a machine-local absolute path patch or publish another vendor fork to bypass dependency publication.
- Add a CI assertion that `cargo metadata` resolves exactly one package ID for each family crate and one winit, and that no `path` dependency points outside the repository.

### 3.8 Vendors outside the UI port

`vendor/hayro*`, `vendor/lopdf` and `vendor/vello_common` are renderer/text-engine vendors. Keep their revisions, patches and tests. Inspect vello package identities and feature unification; incompatible versions may legitimately coexist. Do not force unrelated major versions to unify or edit PDF renderer vendors merely to make `cargo tree -d` empty.

## 4. egui 0.37 breaking changes that touch PdfCraft

| Change | PdfCraft use | Required action |
|---|---|---|
| `default_fonts` no longer a default feature | explicit in ui-egui, pdfcraft, web, craft-ui dev-dep | keep explicit |
| `monochrome_emoji_fonts` gates emoji faces | `✔`, `▲`, `▼` and previously `⏵` text | enable the feature; add `egui-icons.ttf` attribution; keep painted submenu arrows out of font coverage |
| eframe `clipboard` becomes explicit | native eframe with `default-features = false` | enable in native app and native UI tests; web needs none; add feature-identity check |
| `WidgetType` deprecated alias for `Role` | 52 references, 56 `widget_info` closures | map roles explicitly |
| `Id::new` deprecated → `Id::unique` | 74 references | semantic rename |
| `Ui::id()` deprecated → `Ui::scope_id()` | 20 references | mechanical rename; `scope_id()` is the value `id()` returned, not a new inherited ID |
| `UiBuilder::id` deprecated → `scope_id` | `canvas.rs:2607`; not `chrome.rs:697` | rename only the UiBuilder call |
| `Panel` constructors take `AsIdSalt`; panel keys derive from parent scope | 5 panels | migrate 0.36 `PanelState` keys in PR 1 and test the upgrade |
| `Ui::shrink_clip_rect` for panels | all panel content | test clip intersection and overlays |
| `ViewportCommand::Screenshot` callback; `Event::Screenshot` removed | `control.rs` | bounded weak-slot callback; screenshot command itself paints a hidden window |
| `Event::MouseWheel` gains `source: MouseWheelSource` | 41 references | add source; use `Unknown`/`Wheel`/`Trackpad`/`Momentum` only |
| `ScrollStyle.kinetic` / rubber-banding | `fling.rs`, `wheel_pager.rs`, `canvas.rs` | keep PdfCraft fling as document owner; leave egui kinetic for ordinary ScrollAreas; prove one curve |
| `NativeOptions.system_font_fallback` and `system_fonts` feature | not enabled today | set false and guard |
| kittest panics on missing glyphs and unnamed inputs | 204 harness constructions in `tests/it` | fix names/glyphs in PR 1; no global disabling |
| `Event::PasteImage`, `MiddleClickPaste`, PRIMARY | image paste via arboard | adopt the W15P PRIMARY contract; no app-level filtering of the event |
| `Response::accessible_name`, `Ui::label_inputs_by`, `Ui::name_inputs` | custom widgets and text fields | use for accessible names |
| Submenu arrow is a painted `arrow_atom` | test labels `"File ⏵"`, `"View ⏵"`, etc. | update labels/accessible names and keep an arrow-painted assertion |

## 5. Fork patch audit: E1–E21 and E21b (planning-era baseline)

Base: `0.37.0` = `841a328b`. The E1–E21 audit below was made against the planning-era fork head `a4f9a17f`; the current qualification pin is `6d518fc` (see `Current qualification state — 2026-10-25`), and the E21b row records the later pin. The patch table in the fork's `PATCHES.md` is provenance; each row was re-audited against the source at `a4f9a17f` and PdfCraft's needs, so rows naming `a4f9a17f` are planning-era history.

| # | Fork commits | Crate | Upstream status at 0.37.0 | PdfCraft need | PdfCraft tests / removal condition |
|---|---|---|---|---|---|
| E1 | `8f3abf06` | eframe | not upstream; `ViewportHooks` absent | none today: PdfCraft opens one root viewport; `--new-window` is a separate process | compile only; keep for craft-tablet. Remove when upstream adds per-viewport hooks or craft-tablet no longer needs it |
| E2+E3 | `9dd799ab` | egui-wgpu | not upstream; #8361 open | required: `Surface::configure` panic and start-up errors | six `apps/pdfcraft/src/main.rs` surface/GPU guards plus fork-owned behavioral tests; remove when upstream fixes #8361 and the start-up error path |
| E4 | `66bacf23` | egui-winit | upstream #8657 draft; 0.37 still uses smithay | required on Hyprland and every Wayland compositor serving one data device | W15 regular clipboard plus W15P PRIMARY; native Wayland single-data-device test; remove when upstream has the behavior |
| E5 | `0e27d87e`, `777977b7`, `f7e89e23`, `5b2b8e19` | epaint, egui | #8577 open, #8698 reviewed; not in 0.37 | required for Arabic, Kurdish, Hebrew and mixed-direction text | bidi shaping, caret, selection, mark and no-default-fonts tests; remove only when upstream merges #8577 plus fixes |
| E6 | `0abfabcf` | egui | not proposed | required: caret moves must not kill IME composition | text-edit IME focus test; remove when upstream has equivalent |
| E7 | `0670c58a` | egui-winit | not upstream | required: shortcuts on Russian/Arabic/Hebrew Windows layouts | native Windows layout test plus source guard; remove when upstream |
| E8 | `1b03c10f` | egui | not upstream | required: Thai/Vietnamese/Arabic marks above and below the row | TextEdit pixel/ink test; remove when upstream |
| E9 | `b2f5cafc`, `e1341a3d`, `5105330d`, `75a5b3d3` | egui | not upstream | required for Arabic/Kurdish chrome (menus, scroll bars, combo boxes, headers) | RTL chrome geometry tests, no double-mirroring; remove when upstream |
| E10 | `dc90c13e`, `c8e573f7`, `34b2c0d4` | eframe web | not upstream; 0.37 has no `web_keys.rs` | required for the web build on non-Latin layouts and named keys | web build + browser keyboard test; remove when upstream |
| E11 | `92e61afd` | epaint | #8641 draft; not in 0.37 | not required by PdfCraft: epaint 0.2 vello_cpu cannot unify with hayro 0.0.8 | verify with `cargo tree -e features -i vello_cpu`; keep because other suite apps enable multithreading; remove when upstream flushes |
| E12 | `e955bb18`, `485c0b89`, `0185887c`, `7015ac23`, `4067b2c4`, `620e070e` | epaint, egui | not upstream; follow-up to #8577 | required for paragraph direction, RTL alignment, ellipsis at the visual end, UAX #9 L1 and linear reordering | direct direction/ellipsis/L1 tests; remove when upstream |
| E13 | `5e4da7f4` | epaint, egui | not upstream | required for caret/hit-test/Delete/Backspace by grapheme cluster | emoji, combining accent, Devanagari cluster tests; remove when upstream |
| E14 | `cbd77549`, `b3722ac0` | epaint, egui | not upstream | required: fallback faces on the primary baseline and rows growing to fit | replace PdfCraft's manual `y_offset_factor`; baseline/row-height tests; remove when upstream |
| E15 | `f1cae609` | epaint, egui | not upstream | required: `LineBreaker` trait, grapheme-safe emergency breaks, `LayoutJob::language` | CJK/Thai/Hindi line breaking tests; remove when upstream |
| E16 | `b3ef9771`, `bed6dfe3` | epaint, egui | not upstream | required: visual cursor movement with affinity, Home/End in reading order, password mask per cluster, neutral paragraph direction | bidi cursor tests and password-mask test; remove when upstream |
| E17 | `520435df` | egui | not upstream | required: `Context::set_language`, `UiBuilder::language`, AccessKit root language, one text run per bidi level | AccessKit language and text-run tests; remove when upstream |
| E18 | `89de8f3c` | eframe | fork-only | required as the bridge from PdfCraft's language to E17 and web screen reader/IME `lang` | `Frame::set_ui_language` test; remove when upstream |
| E19 | `d64047bf`, `642b74f7`, `baeb4515` | egui-winit, egui | fork-only until winit has the APIs | required for Latin shortcut letters on non-Latin layouts | needs W23/W25; app shortcut test; remove when upstream |
| E20 | `23955535` | egui | fork-only | required so widget text uses the `Ui` language for shaping and line breaking | language/line-break test; remove when upstream |
| E21 | `a70470b0` | egui-winit, egui | not proposed | required for macOS press-and-hold/dictation and Wayland IME replacing exact bytes | needs W26/W27; byte-count IME test; remove when upstream |
| E21b/P7b | `6d518fc799681571963842f95b55a6b68431b15a` | egui-winit, egui | present in the pinned fork; native execution unqualified | bounded outgoing surrounding-text payload, TextEdit producer and Wayland W27 adapter are present in the pinned tree | logical/byte-boundary/password/focus/over-limit tests; native Wayland trace remains open |

### 5.1 Audit notes and traps

- **E2+E3 are not one patch.** The surface fit and the error scopes protect different failures.
- **E4 + W15P.** Regular clipboard uses the winit data device (W15). PRIMARY is a distinct protocol object on the same connection/event queue and per-seat lifecycle (W15P), using SCTK's PRIMARY objects. It must not bind a second regular `wl_data_device`. Preserve password exclusion and no unchanged-selection reclaim. No “filter `MiddleClickPaste`” substitute is acceptable.
- **E21b is present in the pinned tree.** The planning-era candidate lacked the producer; pinned egui `6d518fc` contains the TextEdit producer/`IMEOutput` path and egui-winit calls `set_ime_surrounding_text`. PdfCraft uses ordinary TextEdit, so it needs no custom model adapter; native platform execution and outgoing-text tests remain open.
- **E5/E12/E13/E16 are a dependency chain.** Do not cherry-pick E12 without E5 or E13/E16 without E5.
- **E14 supersedes PdfCraft's baseline hack but not its Windows hinting.** Keep JetBrains Mono `HintingTarget::Smooth`.
- **E15/E20 change line breaking.** Native wrapping must be the only wrapper once active.
- **E17/E20 do not automatically force base direction.** PdfCraft sets `TextDirection::RightToLeft` where the UI locale requires it.
- **E19 is useless without W23/W25; E21 is useless without W26/W27.**
- **E11 is not required by PdfCraft.**

### 5.2 winit patch provenance and compatibility

Base: `v0.30.13` = `e9809ef54b18499bb4f2cac945719ecc2a61061b`.

The retained vendor carries W1–W13. The planning-era reviewed language branch `8b35388b` carried W1–W15 plus W17–W27 but not W15P. W16 is not in this cut. The current pin `a517b384` contains W15P PRIMARY and W27 (`PATCHES.md`); native evidence remains open.

| Group | Patches | Purpose | Upstream / removal |
|---|---|---|---|
| Windows DPI/frame/IME | W1 `b357a332`, W3 `fd104a4c`, W4 `3fffb7ce`, W18 `894bf2d7`, W19 `fa2e1b3d` | mixed-DPI suggested rect, Korean IME cursor, hidden maximized restore, hidden system caret, IME-consumed key presses | W1 merged on master; W3/W4 open; W18/W19 to be proposed |
| Wayland | W2 `13ca693b`, W13 `5506b1a9`, W15 `cf73a29c`+`b05ce129`, W15P (present in the pinned fork), W27 `f40c76a2` | text input owned by the seat, scroll phase policy, per-seat data device and regular clipboard, PRIMARY on the same connection, text-input-v3 incoming delete plus the outgoing surrounding-text API that E21b sends | W2 merged; W13 to be proposed; W15 #4729 open; W15P present in the pinned fork; W27 has master equivalents |
| X11 | W5 `283370ce`, W11 `4df8554b`, W20 `019c95f1`, W21 `23bb19a7` | focused replacement IME context, server DPI fallback, one input context per window, quiet IME spot | W5 open; W11/W20/W21 to be proposed; W20 supersedes W5 |
| macOS | W6 `a82b90fb`, W7 `9b90d5bd`, W8 `6a87addd`, W9 `9500f25d`, W10 `4a7dd791`, W12 `7facf001`, W14 `c488d2e9`, W22 `0816871e`, W23 `c8cd8aa1`, W24 `4b5bc5a0`, W26 `2b9cd1cf` | NSTextInputClient commit flow, discard marked text, emoji picker, Korean space/first key, title-bar drag, cancelable termination, IME ranges, ⌘-layout names, reset, byte-count delete | W14 is opt-in and not enabled by PdfCraft; others open/to be proposed |
| Language/tests | W17 `2be27382`, W25 `eb4007da` | regression source guards; `platform::latin_layout::KeyEventExtLatinLayout` | tests go with their patches; W25 required by E19 |

Compatibility rules:

- The egui fork's `egui-winit` calls `winit::platform::latin_layout::KeyEventExtLatinLayout`, `KeyEventExtMacOS::key_with_command`, `WindowExtMacOS::take_ime_delete_surrounding` and `WindowExtWayland::take_ime_delete_surrounding`; E21b also calls `WindowExtWayland::set_ime_surrounding_text`.
- W14 remains compiled in the shared fork but not opted into by PdfCraft; `craft-quit` remains its terminate path.
- Re-run winit policy/unit tests in the selected fork and app-facing regressions. Native IME/compositor acceptance still requires actual workflows.

## 6. Migration design

### 6.1 Frame boundary and exactly-once side effects

Keep the existing `eframe::App` hooks. Adding a private phase type is allowed only if implementation proves it simplifies real state ownership; do not add a scheduler API or rerun the whole app through a blanket `Context` bridge.

1. `logic` remains the owner of queues, autosave, recovery, file watching, open completion, background polls, control request admission, undo application and other host state. Replayed passes must not re-enqueue an action, duplicate a command or add a second undo step.
2. UI-produced editing commands may apply in the UI, as they do today (`chrome.rs`, `canvas.rs`, `dialogs.rs` call `execute`/`apply_edit`). Preserve promptness. The rule is exactly-once by command/response ID or document generation, not “zero commits on discard”. A discarded pass may repeat layout; it must not repeat the edit.
3. Inventory every UI-side `execute`/`apply_edit`/undo path and add a forced-late-discard test using a real toolbar action. A per-pass pending-command queue keyed by `Response::id`/command identity is preferred over moving all UI side effects into logic.
4. `DocView::begin_render_frame` may collect per-pass demand. Render/upload commits use document/render generation, not `!ctx.will_discard()` as the correctness boundary. A late discard may discard demand work but must not lose an already-produced first-pass texture.
5. `Control::tick` remains in `logic` for queue servicing. It sends `ViewportCommand::RequestPaintWhileHidden` while a screenshot or injected input waits. Injected input is consumed once; a replay does not replay the click.
6. Autosave, recovery, grid-drop timeouts and playback use monotonic time. `Motion::begin_pass` is called from `ui` before panels; no logic-only call consumes `ctx.input` or smooth-scroll state.
7. `Frame::set_ui_language` runs after language resolution, once per pass only when the resolved language changes; it does not read stale input from `logic`.
8. Never infer a logic-only phase from `visible() == Some(false)`; hidden UI paint is a known UI phase and must handle current input.

### 6.2 Panels, docking and persistence

Current integration note: the shared side panes and rail described at the top supersede the former docking and panel layout below. Retain historical requirements as context, not as claims of implemented capabilities.


The first compatibility PR implements an actual one-time panel-key migration. For each of `"tab_strip"`, `"mode_bar"`, `"rail"`, `"rail-nav"` and `"read_right_panel"`:

- old key (0.36): `Id::new(salt)`;
- new key (0.37 root panel): `ctx.viewport_id().root_ui_id().with_salt(IdSalt::new(salt))` (equivalently `parent_ui.scope_id().with_salt(IdSalt::new(salt))` for the root).
- Before the first 0.37 UI pass, if the new key has no persisted `PanelState` and the old key does, read the public `PanelState` and insert it under the new key. Keep the old key untouched for rollback. Do not reset a width that can be migrated.

Test with a synthetic 0.36 memory fixture: write the old key with a known outer width, run the 0.37 app, and assert the panel width survives within 1 point. Dump the old/new IDs for one panel and one child widget. A one-time reset of transient in-panel focus/scroll state is explicitly accepted only if the upgrade test demonstrates it; persisted widths are not reset. PdfCraft's own `docking::Workspace` layout is not touched.

Further tests:

- Panel order: tab strip → mode bar → right rail → central panel. Read-mode inspector must not reorder the rail or duplicate a panel ID.
- A panel's content clip is a subset of the panel rect and parent clip; popups/comment overlays use `Area`/`Window` layers.
- Floating docking, tab detach, group move and split resize survive restart from `Workspace` JSON, not egui memory.
- `DockArea::new(Id::unique("pdfcraft-docking"))`; keep the same salt for craft-ui state. If RTL is selected, pass the explicit direction; persisted split/rectangles/logical tab order stay unchanged.

### 6.3 Viewports

PdfCraft has no deferred or immediate child viewports. Keep `ViewportId::ROOT` as the only production viewport. `window_state` and tests already use it. E1 `ViewportHooks` remains a suite feature, not a PdfCraft requirement. Do not add popout document windows in this port.

### 6.4 Screenshot callback correlation

Replace `ViewportCommand::Screenshot(UserData)` plus `Event::Screenshot` with a bounded, cancelable request record:

- one completion slot per admitted request, owned by that request; the callback holds only a weak reference to the slot;
- callback stores at most one `Arc<ColorImage>` and wakes the owning root viewport after releasing the slot lock; it never encodes PNG, writes a file, runs commands, or strongly captures `Context` in a cycle;
- `Context::request_screenshot` or `ViewportCommand::Screenshot` with the one-shot callback; `finish_pending` takes each completed slot exactly once and replies on the app thread;
- cancellation, timeout and viewport close drop/cancel the slot so a late callback cannot produce a file or second reply;
- admission defaults are 4 requests, 128 MiB/image and 256 MiB aggregate reserved
  in-flight/completed image bytes. These cap pending requests and aggregate image bytes. Reserve checked bytes before issuing a capture, using the allowed surface dimensions; revalidate dimensions before issuance after resize/DPI changes. Release reservations on completion, cancellation, oversize/stale rejection and shutdown; tests assert retained bytes return to zero;
- store issuance-time target-UI logical viewport rect, `pixels_per_point`, requested region
  and geometry epoch in the slot. Reject/retry any resize/DPI/zoom/surface epoch change
  before save, even a change back. Callback arguments contain pixels only. The callback image may be smaller because E2 surface fitting reduced it; derive x/y image scale from the returned dimensions and recorded logical bounds, verify aspect/rounding, and do not use `ctx.pixels_per_point()` at completion. A resize/DPI generation mismatch retries within the deadline or returns a defined error;
- PR 1 waits for the approved craft-ui CL-2F completion-slot helper and adopts it directly; no temporary app-local implementation. Preserve PdfCraft request/path/region/reply policy in `control.rs`.

Hidden-window tests are separate:

- a hidden window with queued injected input needs `RequestPaintWhileHidden` to apply it; without the command the input times out;
- a pending screenshot command itself requests a paint. A request still queued in app
  logic, or awaiting settling before UI issuance, needs `RequestPaintWhileHidden` to reach
  that point; test admission and already-issued capture separately;
- keep late-callback, cancellation, two-region/out-of-order, close, admission overflow and DPI-change tests. Assert exact reply counts, dimensions/pixels, bounded memory and eventual queue drain.

The dependency boundary also requires P2b's bounded asynchronous W15/W15P transfer API
(total deadline, cumulative byte cap, bounded workers/queue, completion wake and cancellation),
and P7's emitted-key identity paired by physical key for repeat/release. PdfCraft adds slow/trickle,
oversized/canceled paste and event-loop responsiveness checks, plus ordinary held-key/Ctrl+Z
layout-switch, release-modifier-first and focus-loss cases. Do not copy those internals locally;
consume the reviewed revisions and assert app-visible behavior. Clipboard shortcut keydown may
be intercepted before egui sees a Key event, so Ctrl+C/Cmd+V alone is not a stuck-key test.

### 6.5 Fonts and glyph coverage

Keep PdfCraft's single-face, last-resort system-font policy. Do not enable eframe's `system_fonts` provider silently. Set `NativeOptions.system_font_fallback = false` and keep a source/feature guard that `eframe/system_fonts` stays off.

Glyph decision: enable `egui/monochrome_emoji_fonts` in `pdfcraft-ui-egui`, the native app and the web dependency graph. This keeps the existing `✔`, `▲` and `▼` text symbols and the OFL/MIT attributed Noto Emoji/emoji-icon fonts deterministic with `PDFCRAFT_SYSTEM_FONTS=0`; do not replace them with new SVG assets in this migration. The new painted submenu arrow is an atom, not text, and is removed from glyph expectations.

Attribution and assets in the first green PR:

- treat the 0.37 font change as a deliberate fixture change: `⋮` (U+22EE) was never covered by the bundled faces, and the craft-libs test fixture moves to `…`; do not restore a tofu-drawing fixture. Production `…`/`−` remain covered;
- update all four `[[bundled]]` `epaint_default_fonts` entries from 0.36.2 to 0.37.0;
- add `egui-icons.ttf`, SHA-256 `122806555c73df7cef55aa1d945289ae27fbc4dbbd31abbb4204ee0cda829cc8`, recorded as the MIT-licensed `emoji-icon-font` subset by John Slegers, with its exact usage;
- run `cargo xtask assets --write` and then `cargo xtask assets`.

Adopt E14 for fallbacks: set `FontTweak::fallback_align = FallbackAlign::Baseline` for script fallbacks; remove metric-derived `y_offset_factor`; keep Windows JetBrains Mono hinting. Add a glyph-coverage test for every UI literal and catalog translation with `MissingGlyphPolicy::Panic` (or `Fonts::has_glyphs`) for embedded-only, craft-fonts, each script fallback, and `PDFCRAFT_SYSTEM_FONTS=0/1`. A missing glyph is a failure, not a snapshot update.

### 6.6 Language and RTL

Current integration note: the shared side panes and rail described at the top supersede the former docking and panel layout below. Retain historical requirements as context, not as claims of implemented capabilities.


End state:

- `logic` resolves the language and calls `frame.set_ui_language(Some(code))` (E18), setting `Context::language` (E17).
- Translated UI text stays logical in catalogs. Remove `catalog.map_translations(display_rtl)`, `bidi::display_rtl` and the matching label preordering. Rely on E5/E12.
- Use explicit `TextDirection::RightToLeft` for `ar`/`ckb` labels whose first strong character may be Latin or a placeholder; use `LeftToRight` for file/path labels where the extension must stay at the end; use `Auto` for document-authored text. A small local direction helper is used only where multiple call sites need it; no new fork-only API.
- Replace `bidi::wrap`/`widgets::wrapped_text` with native `Label`/`LayoutJob` wrapping once E15 is active. A remaining custom wrapper must measure with the same direction/language it paints.
- Set `Style::right_to_left` for `ar`/`ckb` after auditing manual `Layout::right_to_left` call sites. E9 mirrors chrome, not every manually positioned control. Pass an explicit direction to `craft_ui::DockArea`; do not reverse saved workspace order.
- Keep `bidi::visual` only for painter paths that cannot use a `LayoutJob`; never combine it with native bidi on the same text.
- AccessKit labels and copied text stay logical; only painting is reordered.

### 6.7 Input, IME and clipboard

- Add `source` to every `Event::MouseWheel` constructor. The only variants are `Unknown`, `Wheel`, `Trackpad`, `Momentum`. On Wayland, Windows, X11 and web, winit reports `Unknown`; macOS reports `Wheel`, `Trackpad` or `Momentum`.
- PdfCraft's `fling.rs` remains the document-motion owner. On non-macOS platforms it keeps the existing `MouseWheelUnit::Point` + `TouchPhase::End` heuristic, explicitly including `Unknown`; never use the invented `Finger` or `WheelTilt` egui variants. On macOS its custom fling stays off because the OS sends `Momentum`; test that egui kinetic does not add a second document glide. Ordinary widget `ScrollArea`s keep the egui kinetic default; wheel deltas do not create coast velocity, only pointer-drag velocity and momentum do.
- E10/E19/W23/W25: verify shortcuts on non-Latin layouts through synthetic `RawInput` and native Windows/macOS/Linux tests.
- E21/W26/W27: test incoming `DeleteSurroundingBytes` and outgoing E21b surrounding text after delete/commit, deletion-only batches, preedit exclusion, reversed selection, UTF-8 boundaries, long/over-limit selections, field/window switching, password clearing and late serials.
- Text copy/paste stays on egui events; keep `arboard` for image/file clipboard. Enable eframe `clipboard` for native builds. PRIMARY uses W15P: keyboard-only focus, serial freshness, owner/seat changes, empty/absent offers, self-paste, canceled/slow transfers, password exclusion and no unchanged-selection reclaim. Native evidence on wlroots/KWin/Mutter. No second regular data device.

### 6.8 Accessibility

Current integration note: the shared side panes and rail described at the top supersede the former docking and panel layout below. Retain historical requirements as context, not as claims of implemented capabilities.


- Map every `WidgetInfo` role explicitly (`Role::TextInput`, `Role::CheckBox`, etc.).
- Add accessible names to every input widget. 0.37 kittest fails unnamed inputs; use `labelled_by`, `Ui::label_inputs_by`, `Ui::name_inputs` or `Response::accessible_name`.
- Keep logical labels in AccessKit. Do not set preordered visual strings.
- `ControlPlugin` copies the AccessKit tree; `ui.inspect` must keep stable widget IDs across panel migration. Test roles/labels for text field, slider, tab and custom button.
- Panel resize handles expose `Role::Splitter`.

### 6.9 Rendering and GPU

- Keep `surface_texture_limits` and `configure_gpu`. E2+E3 remain required.
- Do **not** skip `DocView::receive` uploads on `will_discard`. A first-pass texture is carried into the final merged `TexturesDelta`; skipping it can lose a page. Use generation identity to avoid re-scheduling renders and avoid duplicate whole-image deltas.
- Assert the final merged `FullOutput.textures_delta.set` contains one whole delta per texture generation; a forced-late-discard test must show the page after the next accepted pass.
- Keep `RESULTS_PER_FRAME`/`UPLOAD_BYTES_PER_FRAME`, hidden-tab suspension, `PDFCRAFT_RENDERER=wgpu|gl`, OpenGL retry and software-renderer notice.
- Screenshot readback is callback-driven; never assume it is ready in the same input frame.

### 6.10 Commands, undo and persistence

No command ID, tool ID, `parity/` entry, undo transaction or saved-settings key changes in this port. Preserve `commands.rs` registry IDs, `Session` document generations and undo steps, `Workspace` JSON, `window_state` storage key `window`, and `persist()` keys `pdfcraft`/legacy `printcraft`. The only persistence migration is egui's panel `PanelState` keys in §6.2.

## 7. PR stages and rollback

All publication is gated by explicit human approval. No push or PR is authorized by this plan.

### Stage 0 — harness and baseline (before production change)

- Land only this document and a test-only measurement driver/harness; no production behavior changes.
- The harness must run on the unmodified baseline `a2ec27b` and, where its adapter permits, stock 0.36.2, stock 0.37.0 and the fork.
- Record baseline frame/idle/RSS samples, `Context::tessellate` with 120 ASCII labels (#8731), startup phase split (instance/adapter/device → pipeline creation → first frame; #8736), and font install/discovery with `PDFCRAFT_SYSTEM_FONTS=0/1` and provider on/off (#8512).
- Record `cargo metadata`, lock hash, vendor hashes, hardware, renderer/driver, DPI, fixtures and actual commands. A baseline may not be measured after production changes.

### Shared fork prerequisites (external to PdfCraft)

The implementation is blocked until the shared plans publish approved immutable revisions containing:

- the reviewed winit W1–W15, W17–W27 and **W15P** PRIMARY path (all present in the current pin `a517b384`);
- the reviewed egui E1–E21 and **E21b/P7b** outgoing surrounding-text integration;
- the craft-libs API-port hunks on current main, with `craft-window` and `craft-print` drift reviewed.

PdfCraft does not vendor a private copy or bypass missing publication with a path patch.

### PR 1 — atomic first-green compatibility change

Depends on: Stage 0 and the complete approved egui/winit revisions (P2b bounded W15/W15P, P7 key release pairing, P7b IME lifecycle, P7c selection fix), plus craft-libs CL-1/CL-2F. PR 4 RTL docking additionally waits for CL-3.

Scope (one reviewable change set; commits may split by file only if every commit is green):

- Pin the egui 0.37 family and winit 0.30.13 fork revision; update `Cargo.lock`.
- Enable native eframe `clipboard` in `apps/pdfcraft` and `crates/ui-egui`; add the feature-identity assertion. State that web has no egui-winit clipboard.
- Enable `monochrome_emoji_fonts`; update `ATTRIBUTION.toml` bundled versions and add `egui-icons.ttf`; run assets write/check.
- Add storytold egui/winit git URLs to `deny.toml` `allow-git`; keep `unknown-git = "deny"`.
- Rename `WidgetType`/`Id::new`/`Ui::id`/`UiBuilder::id`; map roles; fix all kittest accessible-name and missing-glyph failures (204 harnesses).
- Update painted submenu labels: `"File ⏵"` → `"File"`, etc.; update AccessKit/name expectations across `commands.rs`, `theme.rs`, `editing.rs`, `control.rs`, `text_select.rs`, `view.rs`; assert the arrow is painted.
- Add `MouseWheel.source` with the real variants; migrate the screenshot path to the bounded callback; add panel-key migration.
- Port from craft-libs current main using only the reviewed API hunks and `craft-window`; add `craft-window` type reuse and retain `PDFCRAFT_TITLEBAR`/runtime preference semantics. Add `Printer.description` to every literal (19 at baseline) and audit craft-print/GDI/rgb drift.
- Transfer winit/egui-wgpu source guards centrally; replace PdfCraft source-path and `source =` assertions with immutable git graph checks.
- Remove catalog preordering and `display_rtl` as the fork becomes the sole UI bidi owner;
  include the focused logical-text/selection regression in this first fork pin.
- Do not defer names, glyphs, attribution, required bidi adaptation or `deny` to PR 3.

Tests: fmt, clippy `-D warnings`, `cargo test -p pdfcraft-ui-egui`, `cargo test -p pdfcraft`, wasm check, `cargo xtask layers/assets/deny/parity`, native feature/graph assertions, source-guard replacements. Snapshot changes must be reviewed, never auto-accepted.

Rollback: revert PR 1; keep the 0.36 lock and vendor directories in history. Do not leave a 0.37 manifest with a 0.36 lock.

### PR 2 — lifecycle, control and render commit

Depends on: PR 1.

Scope:

- Replace `ctx.input` clocks in autosave/recovery/grid/control timers with monotonic deadlines.
- Move `motion.begin_pass` into UI before panels; remove stale event reads from logic-only iterations.
- Complete exactly-once command/edit/undo handling with a real forced-late-discard test; remove any “no commit on discard” assumption.
- Finish screenshot slot stress: hidden injected input, screenshot command paint, cancel/late/out-of-order/DPI, aggregate reservations.
- Rename `Pending::Frames`/`Shared::passes` to accepted completed frames; increment only when `request_discard_reasons` is empty; add a forced-discard timing test.
- Add generation-safe texture tests; do not gate uploads on `will_discard`.

Rollback: revert PR 2; PR 1 retains a complete 0.37 callback and basic regression coverage.

### PR 3 — language, fonts, RTL and accessibility completion

Depends on: PR 2.

Scope:

- `Frame::set_ui_language`; AccessKit language.
- Expand native wrapping and explicit `TextDirection` tests; catalog preordering and
  `display_rtl` removal already landed in PR 1 and must not be restored independently.
- E14 fallback alignment; glyph coverage; `system_font_fallback = false`.
- `Style::right_to_left` after manual-layout audit; craft-ui `DockArea::direction`.
- Role/name tests already required by PR 1 are strengthened, not introduced here.

Rollback: revert PR 3; PR 2 remains. If native bidi proves wrong, revert the whole 0.37 family to the 0.36 fork tag `767b1cad` with the old preordering.

### PR 4 — platform window, packaging and documentation

Depends on: PR 1 (can run in parallel with PR 2/3 after rebase).

Scope:

- The type re-export/parser replacement lands in PR 1; PR 4 completes extended native policy tests for `craft-window`: `window_controls` re-exports `craft_window::{Button, Layout}`; `decorations` uses `Choice::probe(Prefix::new("PDFCRAFT"))`; remove duplicate parser; keep runtime `Use system title bar` toggle.
- Native Linux/Windows GUI checks, web build, packaging checks and clipboard/PRIMARY/IME workflows.
- Update `vendor/README.md`/`SOURCE.json` only for retained vendors; document removed window-stack copies and exact replacement revisions.
- Update `docs/development.md` and version statements.

Rollback: revert PR 4; PR 1–3 remain.

### PR 5 — performance evidence and final gates

Depends on: all.

Scope: run the Stage 0 harness on the candidate; add paired release results, stock-0.37 control, peak RSS, idle CPU/repaint, texture counters and startup/font/tessellation measurements; update `ROADMAP.md`/status docs. No behavior change.

DAG:

```
Shared forks published -> Stage 0 -> PR1 -> PR2 -> PR3 -> PR5
                                      \-> PR4 -> PR5
```

PR4 may be rebased alongside PR2/PR3, but do not land two PRs that both rewrite `lib.rs` without a rebase review.

## 8. Test plan with user-observable assertions

### 8.1 Existing suites to keep green

- `crates/ui-egui/tests/it` (one binary): accessibility, actions, panels, controls, comments, content, editing, export, fonts, forms, links, pickers, print, recovery, signing, text selection, viewer, XFA.
- `crates/ui-egui/tests/perf.rs`: collapsed comments, 500-page scrolling and panel budget tests.
- `apps/pdfcraft/tests/it`: twelve winit guards (transferred/replaced), and the Wayland scroll-policy compile test.
- `apps/pdfcraft/src/main.rs`: GPU/surface/adapter guards.
- `examples/shot.rs`: headless screenshot of the real shell.
- `crates/ui-egui/src/control.rs`: in-crate control, pointer and multi-pass probes.

Include `crates/ui-egui/tests/it/control.rs`, its discarded-pass requests and `screenshots_of_window_and_region` in PR 1 migration and the existing-suite gate.

### 8.2 New or strengthened assertions

| Area | Test | Assertion |
|---|---|---|
| Lifecycle | `logic_runs_per_pass_but_one_action_commits_once` | Native/web-shaped driver runs logic + UI twice in one frame; one click yields one edit/undo step |
| Hidden logic | `hidden_logic_advances_monotonic_deadlines` | Repeated `run_logic` with advancing wall clock and no new events autosaves once and changes no scroll/zoom |
| Hidden input | `hidden_injected_input_needs_a_ui_pass` | Queued click times out without `RequestPaintWhileHidden`; applies exactly once with it |
| E22 | `completed_multipass_resets_pass_index` | After completed `run_ui`, `current_pass_index()` is zero; a hidden call still runs; no first-pass guard is used as a scheduler |
| Panels | `upgrade_from_036_panel_memory` | Old `Id::new(salt)` width migrates to root-scope key; width within 1 pt; ID dump recorded |
| Panels | `panel_widget_id_stays_stable` | A widget in a panel keeps the same `scope_id`-derived interaction ID across consecutive 0.37 frames; `unique_id` is not substituted for state |
| Panels | `panel_clip_is_intersected_with_parent` | Child clip no larger than panel/parent; overlay uses `Area` and remains visible |
| Docking | `dock_float_detach_and_split_survive_restart` | Same `Workspace` JSON round-trips; panel bodies once; Canvas cannot float/share tabs |
| Screenshots | `two_screenshot_requests_keep_their_slots` | Two regions complete out of order with own dimensions/colours; each reply once |
| Screenshots | `hidden_screenshot_command_paints_itself` | A pending screenshot paints a hidden window; no false `RequestPaintWhileHidden` requirement |
| Screenshots | `late_screenshot_is_dropped_after_cancel` | Timeout/close/cancel produces no second reply/file; reservations return to zero |
| Screenshots | `dpi_change_uses_capture_geometry` | Request-time ppp/rect correlate the image; mismatch retries or errors, never crops with new scale |
| Multi-pass | `discard_does_not_duplicate_edit_or_undo` | One toolbar action with `max_passes=2`: one edit, one undo step, one generation change |
| Texture | `discard_keeps_first_pass_texture_and_deduplicates` | Final merged delta has one whole image per generation; page visible next accepted pass; no upload skip |
| Control | `after_frames_counts_accepted_frames` | Forced discard does not answer early; answers after intended accepted frames |
| Input | `unknown_wheel_source_keeps_wayland_fling` | `Unknown` + Point + End glides on Linux; `Trackpad`/`Momentum` policy covered; no `Finger`/`WheelTilt` |
| Input | `one_finger_lift_glides_once` | Offset after 0.5 s follows `fling.rs`; egui kinetic adds no second document curve |
| Fonts | `every_ui_string_has_all_glyphs` | All `tl!` literals and catalogs pass `MissingGlyphPolicy::Panic` for embedded + craft-fonts + system-font-off |
| Fonts | `fallback_rows_share_the_baseline` | Fallback baseline equals primary; row grows; `y_offset_factor` not the mechanism |
| Language | `arabic_ui_labels_use_an_explicit_paragraph_direction` | With an Arabic face, the label's paragraph direction is `RightToLeft` and its Latin run sits right of the Arabic; English stays `Auto` |
| Language | `arabic_catalog_stays_logical` | Catalog and command lookups stay in logical order and placeholders keep their positions |
| Language | `a_tab_with_an_arabic_file_name_keeps_its_logical_accessible_name` | AccessKit label is logical; paint is visual only |
| Editing | `grapheme_caret_and_delete_are_cluster_safe` | Emoji ZWJ/combining accent move/delete as one cluster |
| IME | `delete_surrounding_bytes_replaces_exactly` | Exact reported bytes deleted; one undo step |
| IME | `outgoing_surrounding_text_is_fresh` | After delete+commit, preedit exclusion, cursor/selection changes, long/over-limit/password/focus cases; E21b payload present and bounded |
| Accessibility | `all_input_widgets_have_names` | Every text input role has non-empty label; no global check disable |
| Accessibility | `panel_tree_roles_and_order` | Tab strip, mode bar, rail and inspector once; expected roles/order |
| Menu | `submenu_arrow_is_painted_not_named` | `"File"` etc. accessible labels; arrow atom visible; no `"File ⏵"` dependency |
| Clipboard | `native_egui_winit_clipboard_is_enabled` | Native graph enables `egui-winit/clipboard`; web does not require it |
| Clipboard | `wayland_text_clipboard_uses_one_data_device` | Regular clipboard works on Hyprland without a second regular device |
| PRIMARY | `primary_selection_uses_w15p` | Password excluded, unchanged selection not reclaimed, serial/seat/offer/self-paste/cancel cases; single data device |
| Selection | `textedit_selection_to_toolbar` | Select a subrange in the paragraph TextEdit, click Bold/colour/underline/spacing in Format and assert only that byte range receives `RangeStyle`, with one undo. Repeat via keyboard/focus surrender. `LineEditor.selection` is populated from cursor_range and used by `LineEditor::style` in `apply_line_format`; right-rail-before-central draw order may protect a path but is not proof. Final fork P7c owns #8763 fix (#8764 remains open/unmerged). #8748 is closed/unmerged and root-only PdfCraft has no child case |
| Web | `web_named_keys_and_non_latin_shortcuts` | Named keys not typed as text; `Ctrl+S` works on Russian layout |
| GPU | existing surface tests | 8600×3423 at 250% fitted to 8192; bad surface errors; pipeline error returns `CreateRenderer` |

### 8.3 Deliberate broken-behavior probes

Before accepting each test, temporarily revert the fix and confirm the named test fails:

- remove `surface_fit` → surface-fit test fails;
- remove `catch_errors` → GPU setup error test fails/panics;
- re-add `display_rtl` to catalog lookups → `i18n::tests::arabic_catalog_stays_logical` fails;
- switch E21 back to character counts or remove E21b payload → IME tests fail;
- enqueue the same UI command on a replayed pass → exactly-once test fails;
- remove the hidden UI paint command for injected input → hidden-input test times out;
- remove an accessible name, or restore `"File ⏵"` as the only label → accessibility/menu tests fail;
- skip uploads on discard → texture/page test fails;
- count passes instead of accepted frames → control timing test answers early.

## 9. Performance protocol and thresholds

### 9.1 Measurement identity

Record for every run:

- exact source SHA, lockfile SHA-256, `cargo metadata` package IDs and sources;
- features (`CRAFT_FONTS_DIR` set/unset, `PDFCRAFT_SYSTEM_FONTS=0/1`, `PDFCRAFT_RENDERER=wgpu/gl`, `monochrome_emoji_fonts`, `clipboard`);
- fixture hash (`tests/perf.rs::big(500)`, `named_comments(10_000)`, panel fixture);
- OS/kernel, CPU, RAM, GPU/adapter/driver/backend, DPI/scale, window size (1400×900), renderer, profile;
- software adapter (llvmpipe/WARP) versus real GPU;
- host load and concurrent builds.

Baseline is measured **before** any production change, on `a2ec27b`, in its own checkout/target/evidence. Candidate runs use the same machine, boot, fixture and environment. The first report and every investigated regression include stock-0.36 and stock-0.37 controls where the harness can run them, with all three identities recorded.

### 9.2 Harness-first method

The Stage 0 harness must exist on the baseline before PR 1. It has three layers:

1. **Frame/idle/memory**: open the 500-page fixture and named-comment fixture; warm up, then alternating paired runs.
2. **Targeted engine measurements**: `Context::tessellate` of 120 ASCII labels for #8731; startup phases (instance/adapter/device → pipeline creation → first frame) for #8736 on WARP and a real GPU; font install/discovery with `PDFCRAFT_SYSTEM_FONTS=0/1` and provider enabled/disabled in an isolated fixture for #8512.
3. **Operations**: open + first frame, zoom, page scroll, dock/float, save/reopen.

Method:

- Build/run only through `maintenance/task.py run/status/finish` with a dedicated task, source and evidence directory; no direct cargo fallback. Submit each build/test directly; do not nest a multi-resource driver.
- Use release binaries and separate processes for baseline/candidate.
- At least 20 warm-up frames, 1,000 measured frames per revision for frame distributions; 10 warmups/30 timed iterations for steady operations; 20 fresh processes for startup. Record every raw sample; report median/p95/p99 and sample counts.
- Alternate baseline/candidate order; use paired runs, separate allocations, no concurrent builds or noisy workloads.
- Measure idle after settling for 5 s: zero repaints after 3 s, process CPU from `/proc/<pid>/stat` or platform equivalent.
- Peak RSS from `getrusage(RUSAGE_SELF).ru_maxrss` or `/usr/bin/time -v`.
- Texture/allocation counters from `FullOutput.textures_delta` and `DocView::receive` upload count/bytes; add test-only counters if needed.
- Never infer UI performance from compile duration; never auto-update a golden or snapshot.

### 9.3 Predefined thresholds

Relative to the paired baseline on the same machine:

| Metric | Acceptable candidate |
|---|---|
| Frame median | ≤ baseline median × 1.05 |
| Frame p95 | ≤ baseline p95 × 1.10 |
| Frame p99 | ≤ baseline p99 × 1.15, with at least 1,000 observations |
| Peak RSS | ≤ baseline × 1.10 |
| Idle CPU | ≤ 1% of one core; zero repaints after 3 s |
| Texture bytes/frame | ≤ baseline × 1.05 + 1 MiB |
| Open + first frame | median ≤ baseline × 1.05; p95 ≤ baseline × 1.10 |
| Operation latency | median ≤ baseline × 1.05; p95 ≤ baseline × 1.10 |

Noise policy: if baseline median varies by more than 3% across repetitions, discard that run and repeat on a quiet machine; never relax a threshold to make a candidate pass. If a candidate fails, re-run once, then profile before deciding. Existing debug `BUDGET = 40 ms` smoke gates remain; stricter existing budgets win. Use the suite's 0.05 ms floor only for sub-millisecond measurements.

## 10. Quality gates and platform matrix

Required for every PR:

- `cargo fmt --check`
- `cargo clippy -p pdfcraft-ui-egui -p pdfcraft -p pdfcraft-web --all-targets -- -D warnings`
- `cargo test -p pdfcraft-ui-egui`
- `cargo test -p pdfcraft`
- `cargo check -p pdfcraft-web --target wasm32-unknown-unknown`
- `cargo xtask layers`
- `cargo xtask assets`
- `cargo xtask deny`
- `cargo xtask parity`
- `cargo xtask ci` in CI
- `cargo tree -d`/`cargo metadata` duplicate/source assertions
- native feature assertion for `egui-winit/clipboard` and `monochrome_emoji_fonts`
- source-guard evidence from the immutable winit/egui-wgpu revisions

Platform rows:

| Platform | Required evidence |
|---|---|
| Linux X11 | real GUI: panels, docking, control screenshot, clipboard, IME (fcitx5/IBus), wheel/zoom |
| Linux Wayland GNOME | app-drawn frame, window controls, resize edges, panel clipping, PRIMARY |
| Linux Wayland KDE/Sway | server-side frame, same layout and shortcuts, regular clipboard |
| Linux Wayland Hyprland | text clipboard/PRIMARY single-device, screenshot control |
| Windows 11 | real GPU and WARP, mixed-DPI drag, hidden maximized restore, >8192 surface, Korean IME, screenshot, craft-print/GDI build |
| macOS | traffic lights/title-bar drag, ⌘Q save prompt via craft-quit (W14 not opted in), press-and-hold/dictation IME, emoji picker, Korean space |
| FreeBSD | CI or explicitly unresolved; no GUI claim without a real session |
| Web (Chromium/Firefox) | `trunk build`, open a PDF, Arabic UI, shortcuts on a non-Latin layout, image paste, no named-key text leakage |

Headless `egui_kittest` is not proof of native focus, IME, file pickers, clipboard/PRIMARY or GPU presentation. A row is either backed by real evidence or listed as unresolved. Record actual selected test counts and failures; zero selected tests are not acceptance.

## 11. Risks and migration traps

1. **Double bidi.** Adopting E5 while keeping `display_rtl`/`bidi::visual` reverses Arabic/Kurdish twice. Remove preordering in the same language PR.
2. **Panel keys.** 0.37 derives panel IDs from the parent scope; widths reset unless the first compatibility PR migrates `PanelState`. Child widget state may reset once and is measured.
3. **`Ui::scope_id` is the old `Ui::id`.** Rename mechanically; do not switch existing `ui.id().with(...)` state to `unique_id()`.
4. **Multi-pass side effects.** `logic` and `ui` run per pass; edits/undo/render queues must be exactly-once. Do not impose “no commit on discard”.
5. **Hidden windows and stale input.** `run_logic` leaves input events/time stale. Current input belongs in the UI; background work uses monotonic deadlines.
6. **E22 pass-index fallacy.** After a completed `run_ui`, `current_pass_index()` is zero; a first-pass guard does not skip ordinary hidden logic. No E22 scheduler API.
7. **Screenshot assumptions.** A screenshot command itself paints a hidden window; injected input needs `RequestPaintWhileHidden`. Callback completion may be off-thread and has no built-in DPI metadata.
8. **Texture uploads.** Skipping uploads on `will_discard` can lose first-pass textures. Use generation identity and the final merged delta.
9. **Kinetic scroll.** Use real `MouseWheelSource` variants; `Unknown` is normal on Wayland/Windows/X11/web. Keep one document glide.
10. **Font provider policy.** eframe's system-font provider can read many installed faces, including proprietary ones; keep it off and preserve `PDFCRAFT_SYSTEM_FONTS=0`.
11. **Default-font glyph loss.** 0.37 gates emoji fonts; enable `monochrome_emoji_fonts`, add `egui-icons.ttf`, and keep the kittest glyph panic.
12. **Kittest stricter accessibility.** Unnamed inputs fail; fix labels rather than disabling checks.
13. **`WidgetType::TextEdit` does not exist as a `Role`.** It is `Role::TextInput`; `Checkbox` is `Role::CheckBox`.
14. **craft-print drift.** The bump adds `craft-winprint-gdi`, `rgb`, `lpstat -l` descriptions and `Printer.description`; audit all, fix 19 literals and preserve current main's security fixes.
15. **Winit source guards.** Twelve app modules read old vendor paths; move them centrally and use exact git identities.
16. **Snapshot churn is not acceptance.** Review every diff and keep semantic pixel tests.
17. **PRIMARY.** W15P is required and is present in the current winit pin; filtering `MiddleClickPaste` does not avoid the protocol risk and is not the selected design.
18. **IME surrounding text.** The planning-era candidate lacked E21b/P7b; pinned `6d518fc` contains the producer. Native Wayland execution remains unqualified, so do not describe W27 as complete integration.

## 12. Resolved decisions and external gates

Decisions made by this plan:

- **E11:** not required by PdfCraft; epaint's `vello_cpu` 0.2.0 cannot unify with hayro's 0.0.8. Verify with `cargo tree -e features -i vello_cpu`.
- **Kinetic scrolling:** PdfCraft's `fling.rs` owns document motion; ordinary widget `ScrollArea`s keep egui's kinetic default. Use `Unknown`/`Wheel`/`Trackpad`/`Momentum`; macOS custom fling is off.
- **Panels:** migrate persisted `PanelState` keys in PR 1; transient in-panel focus/scroll may reset once only if the upgrade test proves it.
- **PRIMARY:** adopt W15P on the existing winit connection/event queue; no app-level filtering and no second regular data device.
- **RTL:** remove catalog preordering; explicit `TextDirection` for UI labels; `Style::right_to_left` after manual-layout audit; explicit craft-ui dock direction; saved workspace order unchanged.
- **Fonts:** `monochrome_emoji_fonts` on; no broad system provider.
- **Window frame:** `craft_window::Choice` at startup; `PDFCRAFT_TITLEBAR=app|system|desktop`; `window_controls` uses craft-window types; runtime preference remains a `ViewportCommand::Decorations` toggle.
- **Screenshot helper:** PR 1 waits for and directly adopts the reviewed craft-ui helper after CL-2F; no local duplicate.
- **Performance:** harness and baseline precede production; stock-0.37 control accompanies first baseline and regressions.

External gates that cannot be decided from this host:

- final approved immutable storytold egui/winit/craft-libs revisions, including W15P and E21b/P7b;
- native Wayland PRIMARY/IME evidence on wlroots/KWin/Mutter;
- Windows real-GPU/WARP and native GUI evidence; macOS and FreeBSD hardware evidence;
- browser runtime and non-Latin keyboard evidence;
- explicit human approval of every branch/tag push, PR or publication.

## 13. Limitations of this plan

- No build, test, clippy, fmt, wasm, GUI or performance run was performed. The current tree was read only.
- No compiler diagnostics were generated for this baseline. Historical audit diagnostics are stale.
- Upstream status comes from the provided `upstream-discussions.json` snapshot and local refs; it is not a live GitHub query. #8763 and #8764 are an open issue/PR, not release fixes. #8748 is closed and N/A because PdfCraft uses only `ViewportId::ROOT`.
- No native Wayland compositor, macOS or Windows session was available; PRIMARY, IME, focus and real-GPU behavior remain unverified.
- No performance baseline has been measured. The thresholds in §9 are predeclared relative limits; the absolute baseline must be recorded before implementation.
- The exact publication revisions for egui, winit and craft-libs remain external prerequisites, not user choices.
- Only `docs/egui-037-migration-plan.md` was written. No production file, manifest, lockfile, vendor tree or test was changed.

## 14. Source index used for this plan

PdfCraft:

- `Cargo.toml`, `Cargo.lock`, `deny.toml`, `ATTRIBUTION.toml`, `xtask/src/assets.rs`.
- `apps/pdfcraft/Cargo.toml`, `apps/pdfcraft-web/Cargo.toml`, `apps/pdfcraft/src/main.rs`, `apps/pdfcraft/src/decorations.rs`, `apps/pdfcraft/tests/it/*.rs`.
- `crates/ui-egui/Cargo.toml`, `crates/ui-egui/src/lib.rs`, `control.rs`, `canvas.rs`, `chrome.rs`, `window_controls.rs`, `window_state.rs`, `theme.rs`, `system_fonts.rs`, `motion.rs`, `recovery.rs`, `fling.rs`, `wheel_pager.rs`, `wheel_boost.rs`, `scroll_speed.rs`, `bidi.rs`, `widgets.rs`, `print_ui.rs`.
- `crates/print/src/tests.rs`, `crates/ui-egui/tests/it/*.rs`, `crates/ui-egui/tests/perf.rs`, `crates/ui-egui/examples/shot.rs`.
- `vendor/README.md`, `vendor/winit`, `vendor/egui-wgpu`, retained `vendor/hayro*`, `vendor/lopdf`, `vendor/vello_common`.

Target/reference source:

- egui 0.37 `841a328b`, planning-era candidate `a4f9a17f` (current pin `6d518fc`): `Context::run_ui`/`run_logic`/`end_pass`/`current_pass_index`, `Ui::scope_id`/`unique_id`, `UiBuilder`, `PanelState`/`Panel::id`, `ScreenshotCallback`, `MouseWheelSource`, `ScrollStyle`/`ScrollArea`, `menu::SubmenuButton::arrow_atom`, `epaint_default_fonts`.
- winit vendor 0.30.13, planning-era candidate `8b35388b` and current pin `a517b384`: `PATCHES.md`, `platform/wayland.rs::set_ime_surrounding_text`, per-seat clipboard/data-device APIs.
- craft-libs `3832c47` (planning-era; ancestor of the current `25da131` pin), PR 199 `b68de8c`/`8614e4a`/`ec8840a`: `crates/ui`, `crates/window`, `crates/print`, `docs/egui-037-suite-rollout.md`.
- Shared evidence: `lifecycle-source-audit.md`, `ime-surrounding-gap.md`, `upstream-discussions.json`.


## Local implementation update — 2026-10-23

The user clarified that reviewed plans remain local and implementation proceeds before publication. The earlier requirement to publish/land this plan first is superseded; pushes and PR publication still require explicit approval.

The owned checkout now HEADs at `a2ec27b4775f53c40cad3456936d9406611e3a8d`; `57e531ccca0047597809748b5cb4276336d6051a` and the earlier baseline SHAs above remain historical review evidence, and both refs remain reachable. Current integration uses this checkout and its qualification lock; paired measurements must record the exact source and lock they used.

Preserve document-folder native dialogs, colour popup close policy, panel-header spacing, bookmark contrast, and org_drag cancellation after document changes. Preserve new integration regressions and generate fresh baseline geometry.

The complete static delta review and exact old/new object identities are retained in the task evidence: `canonical-diffs-refresh-20261023.json` and `overnight-consumer-review-20261023.md`. This amendment claims no new build, native GUI or performance acceptance.

## Local implementation amendment — 2026-10-24

**Historical note (superseded by the 2026-10-25 qualification state).** The migration is implemented locally but remains unpublished. At the time of this amendment, `egui`/`eframe`/`egui-wgpu` 0.37.0, the reviewed winit worktree and the shared craft-libs helpers resolved through `local-integration-config.toml`; the checked-in `Cargo.toml`/`Cargo.lock` did not yet pin them. The current tree pins egui `6d518fc`, winit `a517b38` and craft-libs `25da131` (craft-ui, craft-quit and craft-print) directly, while `craft-docpath` remains at `82a24b006eefa036b43c09a67fb9a3fbd17ffe63` (see the top section). The repository quality gates passed as recorded below; publication still requires fresh human approval. No push, PR or release is authorized by this amendment.

The font contract now includes the native-only Simplified Chinese face: the pinned craft-fonts input supplies `fonts/simplified-chinese-manifest.txt` (LXGW WenKai Lite, OFL-1.1), `crates/fonts/build.rs` embeds it on native builds only under a 16 MiB cap, and a `CRAFT_FONTS_REQUIRED` build fails rather than skipping an absent or over-cap required Chinese face. Traditional Chinese is served by an installed `Hant` face placed before the Japanese faces even when LXGW is embedded. CI, release and FreeBSD keep an immutable craft-fonts commit pin; the current candidate is recorded in the task evidence and the pin must be updated only to a published central commit.

Fallback faces now use egui 0.37's `FontTweak::fallback_align = FallbackAlign::Baseline` for the existing one-baseline intent (the metric-derived `y_offset_factor` compensation is gone). Control-channel synthetic input that times out after a press keeps one bounded release batch, so a key, pointer button or modifier cannot stay held when the window resumes. Accepted-frame counting remains one count per presented frame, verified at `max_passes` above two.
