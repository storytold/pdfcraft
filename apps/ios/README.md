# PdfCraft iOS Proof of Concept

This branch contains an experimental iOS/iPadOS integration of the open-source PdfCraft engine.
The goal is to evaluate a native mobile architecture using the existing PdfCraft Rust core with a Swift/SwiftUI interface.
This project is not an official ArtCraft mobile release and does not imply endorsement by ArtCraft.
The implementation is submitted for technical evaluation in
[storytold/pdfcraft#7549](https://github.com/storytold/pdfcraft/pull/7549).
The app is configured as a universal iPhone and iPad app; its current native workflow is tested
on both Simulator device families. This does not claim complete feature parity with desktop PdfCraft.

[Blackout Secure](https://blackoutsecure.app) contributed this experiment.
The installed app displays **PdfCraft**, using the same lion artwork as macOS.
The development target/executable remains **PdfCraftMobilePOC**. Its unregistered
POC bundle identifier is defined once in [project settings](project.yml), not an
authorized production Apple identifier.
This is not a production release or an Adobe application, and contains no copied Adobe
code, graphics, fonts, or UI assets.

## Upstream language-policy proposal

The contributor explicitly approved a small Swift/SwiftUI Apple-platform experiment,
despite the upstream Rust-only native-app policy. The proposed exception is tracked in
[storytold/craft-contrib#63](https://github.com/storytold/craft-contrib/pull/63).
It is **not** upstream approval and does not change the synchronized house rules in
[AGENTS.md](../../AGENTS.md). Upstream adoption requires maintainer agreement.

## Relationship to the suite proposal

The suite-wide direction is under discussion in
[RFC-0039](https://github.com/storytold/craft-contrib/pull/60). That proposal currently
favors an egui-based Mobile Mode, Android first, with iOS later. This SwiftUI shell is a
separate iPhone/iPad proof of concept and is evidence for that discussion, not
an approved suite architecture or a claim of full feature parity. Do not copy this shell
or its Rust bridge into other Craft apps without the required maintainer decisions.

Related upstream work is tracked in the
[PdfCraft mobile-support request](https://github.com/storytold/pdfcraft/issues/2085),
the [draft native mobile host](https://github.com/storytold/pdfcraft/pull/1119),
the [shared `craft-mobile` proposal](https://github.com/storytold/craft-libs/pull/632),
and the [shared-crate candidate inventory](https://github.com/storytold/craft-libs/issues/115).
These proposals use different approaches and remain separate from this experiment.
The Tauri/WASM host in #1119 covers Android and more editor functionality; this
Rust-raster/native-view experiment does not replace it or claim its language coverage.
Earlier UniFFI/scaffold alternatives
[#5617](https://github.com/storytold/pdfcraft/pull/5617) and
[#2740](https://github.com/storytold/pdfcraft/pull/2740) are closed. The Android suite
[#5437](https://github.com/storytold/pdfcraft/pull/5437) was explicitly declined for
current upstream Android scope. Those decisions are not reversed by this POC.

PDF parsing, rendering, text extraction, page rotation, validation, serialization, resource limits, and
document ownership stay in Rust. Swift is limited to native views, accessibility labels,
Files access/coordination, presentation state, export/share sheets, and the typed bridge.
There is no Android target, web wrapper, WebView, Electron, or separate PDF implementation.

## Shared icon and package identity

The universal [AppIcon asset](PdfCraftMobilePOC/Assets.xcassets/AppIcon.appiconset)
reuses the openly licensed [macOS artwork](../../assets/app-icon/README.md).
[The icon conversion](../../packaging/ios-icon.swift) removes the macOS-only
transparent inset and fills the corners with the same red, producing an opaque
1024x1024 image for iOS's own mask. It is not a new mobile logo; light/dark/tinted
variants are not separately designed. Both iPhone and iPad use this icon.
Its provenance and hash are recorded in the root asset manifest.

[Project generation](../../scripts/generate-ios-project.sh) builds the ignored
`target/ios/Info.plist` by combining the [iOS-only configuration](Config/Info.plist)
with an explicit whitelist of compatible fields from
[the canonical macOS template](../../packaging/macos/Info.plist.in). Display name,
development region, copyright and version/build fields are inherited, not maintained
as separate iOS copies. Missing shared fields stop generation with an error.
The [isolated Apple tests](../../scripts/test-apple-build.sh) exercise inheritance,
XML escaping, prerelease version mapping, desktop-field exclusion and explicit
missing-field/tool-version failures. Aggregate Simulator tests run them once before
the native suites; the native test command also compares compiled identity/version
metadata with its desktop/generated sources.
When using `CRAFT_FONTS_DIR`, project generation reuses packaging's
`copy_font_licences` helper to stage the embedded fonts' OFL notices as app resources.
Use the same input for Rust compilation and project generation. Regeneration
removes stale generated notices when that input changes or is unset.
[Project settings](project.yml) consume that generated metadata.
Generation also reuses [packaging's shared setup](../../packaging/env.sh) for
the workspace version (or `PDFCRAFT_VERSION`) and `PDFCRAFT_BUILD_SHA`. The numeric
version is used as `MARKETING_VERSION`, with full version/build commit retained as
`PdfCraftVersion` / `PdfCraftBuildCommit`. Regenerate the project after changing
those inputs; do not invoke XcodeGen directly without the generation script.
`CURRENT_PROJECT_VERSION` remains the separate POC build number `1`, not a
production distribution counter. Licence texts and `NOTICE` are bundled too.

The unregistered POC bundle ID, internal bundle name, target/module/executable, universal device families and visible
POC disclaimer remain unchanged. The production macOS identifier, desktop category,
minimum system version, Finder document handlers, and desktop translation list are
not copied: the mobile shell has not implemented those integrations/translations.
The native Files folder now displays **PdfCraft**, matching the installed app name.
This alignment does not make the upstream review proposal an official release or enable signing or uploads.

## Desktop-first native workspace

The mobile shell follows the macOS app's information architecture instead of designing
a separate product: **Welcome to PdfCraft**, **Recommended tools**, **Pinned Tools**,
**All tools**, and **Find tools and commands**. It adapts:

- [desktop Home](../../crates/ui-egui/src/home.rs) for opening, recommendations and pins;
- [tool navigation](../../crates/ui-egui/src/panels/tools.rs) and
  [tool search](../../crates/ui-egui/src/panels/all_tools_ui.rs) for discovery;
- the engine's [catalogue](../../crates/engine/src/catalog.rs) and
  [command registry](../../crates/engine/src/commands.rs) for terminology and workflow;
- [desktop theme tokens](../../crates/ui-egui/src/theme.rs) for neutral light/dark
  chrome, blue accents, cards and a white PDF page.

One small [native tool list](PdfCraftMobilePOC/WorkspaceView.swift) drives Home cards,
the sidebar, pinning and search. [ContentView](PdfCraftMobilePOC/ContentView.swift)
routes those choices to the existing Rust-backed workflow. Choose a tool before opening
a PDF and Files opens first, then the selected workflow resumes. Organize pages offers
page selection and the existing explicit Rotate action, not an automatic edit or the
desktop's full page-grid editor.

iPad uses a collapsible **All tools** / **Pages** sidebar. Compact layouts, including
iPhone, use the same tools in a searchable sheet. **Open file** remains near the top
of Home. The navigation bar shows the document name; page position and unsaved changes
stay visible in the document workspace. Keyboard shortcuts include Command-O on Home,
Command-K for tools, Command-W to close, and Shift-Command-S to save a copy.

Use **Appearance** to choose System (the default), Light or Dark. Appearance and pinned
tool IDs persist locally; no document paths or contents are saved in these preferences.
The palette is a deliberately small native mirror, not an automatically generated
copy of the egui theme. Platform text, controls, sheets and touch targets remain native.

The reuse boundary is intentional: PDF rules live in the existing
[Rust engine](../../crates/engine/src) and [automation API](../../crates/automation/src),
reached through the [mobile Rust adapter](../../crates/mobile/src/lib.rs) and generated
bindings. Extend a shared Rust operation before adding a new mobile document workflow;
do not copy the desktop's PDF implementation into Swift. The remaining small native
adapter owns supported-tool selection, platform wording/SF Symbols, and the colour
mapping above. Do not copy the full desktop tool catalogue, parse Rust source text
to generate native UI, or pull egui into the engine merely to share presentation
constants. The existing native tool list already serves Home, pins, search and sidebar.

Only the currently working mobile tools are offered: Organize pages, Read Text,
Save a Copy and Share. Desktop-only Edit/Convert/E-Sign tabs, community/trademark
assets, pinned folders and recent-file storage are not copied as nonfunctional UI.
Files already provides recents, folders and provider access. This keeps the development
surface small without introducing a second PDF engine, a new library, or a switch to an
unapproved shared host. Future changes follow the scoped [iOS rules](AGENTS.md).

## Unsupported controls and future Apple distribution

The dated [POC gap inventory and publishing guide](PUBLISHING.md)
records unsupported desktop controls and distinguishes this implementation from
the older published POC baseline. It covers Apple organization membership, secure signing
and API credential placement, private TestFlight, external beta review, App Review,
release blockers and manual release.

Shared CI is proposed in
[storytold/craft-common-actions#259](https://github.com/storytold/craft-common-actions/issues/259)
and [storytold/craft-common-actions#260](https://github.com/storytold/craft-common-actions/pull/260).
No publishing workflow, credential or store submission is activated. Existing desktop
CI/release behavior is unchanged; this remains an upstream-submitted proof of concept.

The [Apple platform review](../../docs/apple-platform-review.md) now records shared
repository ownership, the aggregate macOS+iOS build/test/unsigned-package command,
Store readiness blockers and proposed user-visible changes. The current desktop
DMG is not a Mac App Store product; signing it differently is not sufficient.

## Architecture

```text
SwiftUI / UIKit Files and share sheets
                |
       DocumentBridge actor
                |
   generated Swift / C / Rust bindings
                |
 pdfcraft-mobile: one serialized Rust worker
                |
 Automation::call -> engine Session -> existing PDF crates
```

The existing renderer is used, not PDFKit or a preview placeholder.
The facade wraps the existing `doc_open`, `page_render`, `text_extract`, `page_rotate`, `doc_save`, and
`doc_close` operations rather than duplicating PDF semantics.
`Automation::with_javascript(false)` disables document-supplied scripts before opening.
Automation's MCP/network-facing feature is disabled.

The bridge generator is pinned to **swift-bridge 0.1.59** (MIT OR Apache-2.0).
UniFFI was evaluated but its MPL-2.0 license is outside the current dependency allowlist.
Changing that policy requires separate approval. Bindings are generated, never edited.
The borrowed-path `open` operation returns a typed JSON success/error envelope because
this generator version cannot produce a throwing Swift closure for a borrowed Rust string.
Other operations use generated throwing results. Handwritten Rust remains safe; the
workspace `unsafe_code = forbid` policy is unchanged.

One Rust thread owns all non-Send engine state. A bounded request channel serializes work;
Swift confines the generated non-Send session to an actor. JSON metadata, image bytes, and
URLs cross actor boundaries, not engine pointers. A last-resort panic boundary closes the
worker after an engine panic instead of reusing possibly inconsistent state.

## Prerequisites

- Apple Silicon macOS with the full Xcode toolchain selected, license/first-launch setup
  completed, and an installed iOS Simulator runtime. The Xcode IDE need not remain open.
- Workspace Rust toolchain (Rust 1.95 minimum), Cargo, Python 3, and **XcodeGen 2.46.0**.
- Rust targets: `rustup target add aarch64-apple-ios-sim aarch64-apple-ios`.
- Recommended VS Code extensions: Swift (`swiftlang.swift-vscode`), rust-analyzer
  (`rust-lang.rust-analyzer`), Copilot (`GitHub.copilot`), and SweetPad (`sweetpad.sweetpad`).
  Use the Swift extension/SweetPad's supported LLDB integration for native debugging.
- Device installation additionally needs an Apple development signing identity, team,
  provisioning, and a trusted device with Developer Mode enabled.

The app deployment target is **iOS/iPadOS 17.0**, arm64 only. The Rust build explicitly
uses the same deployment target, including C dependencies.

## VS Code / command-line workflow

Run commands from the repository root. Equivalent `iOS:` tasks are in
[.vscode/tasks.json](../../.vscode/tasks.json).

```sh
# Real Rust static library, generated Swift/C bindings, generated Xcode project, app:
bash scripts/build-ios-simulator.sh

xcrun simctl list devices available
bash scripts/run-ios-simulator.sh <simulator-uuid>

# Run the full Rust, hosted Swift, and native UI workflow test suites on each device family.
# Use an available iPhone UUID for the first run and an available iPad UUID for the second.
# Run one Simulator at a time; the scripts share generated bindings and build output.
bash scripts/test-ios.sh <iphone-simulator-uuid>
bash scripts/test-ios.sh <ipad-simulator-uuid>

# Launch interactively on either device family using that Simulator's UUID:
bash scripts/run-ios-simulator.sh <simulator-uuid>

# Individual generation/build steps:
bash scripts/build-ios-rust.sh aarch64-apple-ios-sim
bash scripts/generate-swift-bindings.sh aarch64-apple-ios-sim
bash scripts/generate-ios-project.sh

# Physical-device compile/link check, without signing or installation:
bash scripts/build-ios-rust.sh aarch64-apple-ios
xcodebuild -project apps/ios/PdfCraftMobilePOC.xcodeproj \
  -scheme PdfCraftMobilePOC -sdk iphoneos -destination 'generic/platform=iOS' \
  -derivedDataPath target/ios/DeviceDerivedData CODE_SIGNING_ALLOWED=NO build

cargo fmt -p pdfcraft-mobile -- --check
cargo clippy --locked -p pdfcraft-mobile --all-targets --features swift --no-deps -- -D warnings
cargo xtask layers
cargo xtask assets
cargo xtask wasm
```

Generated bindings/build products live under ignored `target/ios/`; XcodeGen's
`PdfCraftMobilePOC.xcodeproj` is ignored. The source of project configuration is
[project.yml](project.yml), [Config/](Config/), and the build scripts.
Each native test invocation prints a unique `target/ios/TestResults/<device>.*`
directory containing `hosted.xcresult` and `ui.xcresult`; explicit result paths
prevent Xcode's DerivedData retention from pruning earlier device evidence.
Inspect summaries with `xcrun xcresulttool get test-results summary --path <result>`
and export attachments with `xcrun xcresulttool export attachments --path <result>
--output-path <directory>`. Keep full native-chrome captures private under the
current asset policy; the native-resource publication exception is requested,
not granted, in [RFC-0041](https://github.com/storytold/craft-contrib/pull/63).
Only permitted synthetic document-canvas crops are published as current evidence.
Build scripts must run before building the generated Xcode project.
Intel Simulator and XCFramework distribution are not implemented.
Release now selects the matching optimized Rust archive. Use the
[aggregate Apple driver](../../scripts/build-apple.sh) for macOS+iOS builds and
unsigned packaging; its verified commands/products and separate signing/Store
blockers are in the [central review](../../docs/apple-platform-review.md).
Build/archive success does not enable signing, TestFlight or Store submission.

Use SweetPad to select the generated project, scheme, destination, and debugger.
Only open the Xcode IDE for account/signing/provisioning or Apple-specific inspection
that cannot reasonably be done through VS Code and CLI tools.
The run script boots the selected Simulator and installs/launches the app; it does not
keep a terminal watcher running. Stop the app with
`xcrun simctl terminate <simulator-uuid> <bundle-id-from-built-Info.plist>`, and shut down
an otherwise unused Simulator with `xcrun simctl shutdown <simulator-uuid>`.

## First vertical slice

1. **Open PDF** uses the native Files picker. Security-scoped, coordinated access stays
   active until Rust has made a bounded private snapshot; the original is never edited.
2. View locally rendered pages. Pages are loaded lazily, images are released on
   disappearance, and pinch/Zoom controls change their displayed size (maximum 3x).
   On iPad, the sidebar selects pages. Tap a page label to choose the page to rotate.
   **Read Text** opens that page's embedded text in a selectable, scrollable native sheet.
   Use the selection menu to copy text, or VoiceOver to read it. Extraction is on demand,
   uses the Rust engine's reading order and copy permissions, and stays local.
   Empty pages explain that there is no extractable text; image-only scans need OCR,
   which this shell does not implement. Extraction errors are shown, never treated as
   empty pages. A visible warning marks text cut at engine or mobile limits.
3. **Rotate** applies a clockwise 90-degree engine edit to the selected page and rerenders.
4. **Save a Copy** asks Rust to validate and atomically stage a new PDF, then opens the
   native Files export picker. Cancelling export retains the UI's unsaved-change warning.
   The engine's staged-copy dirty state and successful external export are distinct.
5. Close and reopen the exported copy through Files; rotation must persist.
6. **Share** passes a current, validated PDF copy to the native activity sheet. It does
   not require a separate Files export first and does not dismiss an unsaved-change warning.

Hosted Swift tests generate original rectangle-only, embedded-text and two-page PDFs,
plus deliberately invalid input, in code. They stage
`PdfCraft iOS UI Fixture.pdf`, `PdfCraft iOS Text Fixture.pdf`,
`PdfCraft iOS Multipage Fixture.pdf` and `PdfCraft iOS Invalid Fixture.pdf` in the test app's
`Documents/PdfCraft UI Workflow Test/` folder for the subsequent UI tests.
The text fixture includes two lines and a non-ASCII character, without embedding a font file.
That test-owned directory's previous `copy-2.pdf`
export is removed before each complete run; do not put personal documents there.
No binary fixture, font, or image asset is committed. UI tests use an English Simulator,
test real Files import/export, check portrait-to-landscape rotation and reopening, and
present the native share sheet. Text UI tests check extracted text before/after rotation
and the no-text state on rectangle-only pages. Run both test targets in order with the script.
The app's Documents folder is visible in Files; private working copies/previews are not
stored there.

The project targets both iPhone and iPad (`TARGETED_DEVICE_FAMILY` is `1,2` in
[project.yml](project.yml)). Run one Simulator at a time; the scripts share generated bindings
and build output. UI tests exercise the iPhone and iPad Files picker behavior, including the
iPad sidebar and its native export-picker controls.
Native UI tests allow up to five minutes per test, including bounded waits for Files
provider startup. The test adapts to iPhone sheet dismissal and iPad's native Cancel
control rather than replacing either platform's Files UI.

## Verified locally

Current aggregate/native/optimized-device results and quality-gate limitations
are recorded once in the
[Apple platform review](../../docs/apple-platform-review.md#validation-evidence).
The following dated entries preserve the earlier incremental POC evidence, not
the current test counts or a production-readiness claim.

### Initial baseline (2026-11-15)

Validation on 2026-11-15 used Apple Silicon, Xcode 27.2, Swift 6.4.2, Rust 1.99.1,
XcodeGen 2.46.0, and the iOS 27.0 Simulator runtime.

| Check | Result |
| --- | --- |
| Rust real-engine workflow tests | 4 passed |
| Hosted Swift tests, iPhone 18 Pro | 3 passed |
| Native Files/rotate/export/reopen/share UI workflow, iPhone 18 Pro | 1 passed |
| Hosted Swift tests, iPad Pro 11-inch (M5) | 3 passed |
| Native Files/rotate/export/reopen/share UI workflow, iPad Pro 11-inch (M5) | 1 passed |
| Simulator Rust library and native app build | Passed |
| Device Rust library and unsigned arm64 iPhoneOS app compile/link | Passed |
| Mobile Rust formatting and scoped strict Clippy | Passed |
| Workspace layering, asset, licensing-metadata, and wasm gates | Passed |

Each hosted Swift suite includes two behavior tests and one synthetic Files-fixture
preparation test. Both UI runs verify that cancelling export retains the unsaved-change
warning, a successful export can be reopened with rotation preserved, and the native
share sheet can complete its Copy action. All final test commands exited successfully;
the complete `scripts/test-ios.sh` entry point also passed on iPad.
XCTest result bundles remain under `target/ios/*DerivedData/Logs/Test/`.

The iOS 17 deployment setting is a compile-time target, not a claim that iOS 17 was
runtime-tested. No physical iPhone/iPad installation or signed archive was verified:
there were no configured development signing identities. Configure your Apple Developer
account/team and provisioning locally, then repeat the same workflow on real hardware.
Do not commit signing certificates, provisioning profiles, or account details.

### Text reading validation (2026-11-16)

The complete `scripts/test-ios.sh` workflow passed on both Simulator families with
the toolchain above. Each run passed **7 Rust tests** (2 payload-boundary tests and
5 real-engine workflows), **4 hosted Swift tests**, and **2 native UI tests**.
The new tests cover per-page Unicode text, empty pages, invalid/closed requests,
rotation/reopen, stale-document isolation, UTF-8 limits, and the native text sheet.
The focused reader UI test also passed with a long-press selection and native
**Copy** action on both devices.
The existing Files/rotate/export/reopen/share workflow still passes on both devices.
Scoped strict mobile Clippy and Rust formatting also passed.

The text appears as native accessible text in the UI tests. VoiceOver speech itself,
physical-device behavior, and scanned-document OCR were not tested or added.
The previously noted Rust-worker QoS runtime warning remains.

### Desktop-first workspace validation (2026-11-16)

After the native workspace adaptation, the complete `scripts/test-ios.sh` entry point
passed sequentially on both device families:

| Simulator | Rust tests | Hosted Swift tests | Native UI workflows |
| --- | --- | --- | --- |
| iPhone 18 Pro, iOS 27.0 | 7 passed | 4 passed | 3 passed |
| iPad Pro 11-inch (M5), iPadOS 27.0 | 7 passed | 4 passed | 3 passed |

The workspace workflow covers search filtering, pin and Dark appearance persistence
across relaunch, tool-first Files import, the Organize panel with a visible PDF, and
returning Home. Existing text selection/Copy and Files/rotate/export/reopen/share
workflows also pass. The compact page selector has a bounded, text-size-scaled height;
page lists remain lazy.

Visual inspection confirmed the dark Home layout on both families, the iPad tool
sidebar, and the iPhone's white rendered PDF against neutral chrome. Light appearance,
all Dynamic Type sizes, split-view sizes and VoiceOver speech were not exhaustively
visually validated. These results remain Simulator evidence, not physical-device
certification or full desktop feature parity.

Native tests accommodate the compact search UI hiding Done during active search and
use a double synthetic thumbnail tap on iPhone because its Simulator Files sheet can
ignore a single synthetic tap. This changes test interaction, not the app's Files
workflow. Run the complete script when repeating export tests: hosted fixture setup
removes the previous test-owned export before UI tests start.

### Device Hub operational validation (2026-11-16)

The expanded complete `scripts/test-ios.sh` entry point passed sequentially on the
same Simulator devices displayed by Xcode 27 Device Hub:

| Simulator | Rust tests | Hosted Swift tests | Native UI workflows |
| --- | --- | --- | --- |
| iPhone 18 Pro, iOS 27.0 | 7 passed | 5 passed | 6 passed |
| iPad Pro 11-inch (M5), iPadOS 27.0 | 7 passed | 5 passed | 6 passed |

Both commands exited 0; native result summaries reported no failures or skips.
New coverage exercises:

- Tool-first import cancellation without an error or stale pending tool, malformed
  PDF error presentation and subsequent successful open/read/close.
- Two-page selection and scrolling, page-specific text, Zoom in/Fit page with
  actual image-size checks, rotating only page 2, portrait/landscape controls and
  sharing without clearing the unsaved-changes warning.
- Largest accessibility text size (`Accessibility XXXL`): opening through Files,
  visible document/read controls, actual Save a Copy/export cancellation, text
  reading and closing. A toolbar's transient/incorrect `isHittable` value is not
  used as a substitute for performing the export.
- A real Swift/Rust two-page save/reopen round trip: page 1 remains pixel-identical,
  page 2's rotation persists, and the original source bytes are unchanged.

Zoom controls now announce their page number and current percentage. Files test
helpers scroll thumbnails fully into view before coordinate taps and hold the
scroll gesture before release to avoid selecting during deceleration. Export
cancellation uses iPad's native Cancel and iPhone's sheet-dismiss gesture, then
waits for the actual export navigation bar to disappear.

Device Hub is the display/control host for these same native Simulator destinations;
the repeatable operational flows use XCTest/XCUITest, not host-window coordinates
or a replacement Files/share implementation. Each UI result bundle retains
landscape and accessibility-text screenshots for review in Xcode. Device Hub Home
layouts were also inspected directly. This is not exhaustive visual inspection of
every operation, split-view width, text size or VoiceOver speech.
Build/test output still reports the existing unused Rust patches/vendored import
and Xcode debugger-version warning. A separate console log was not available from
the result bundle; continuous live console monitoring is not claimed.
No physical-device, memory-profile, signing or distribution validation was added.

### Icon and bundle metadata validation (2026-11-16 follow-up)

After adding the shared lion icon and compatible macOS metadata, the canonical
script passed on both Simulator families: **7 Rust tests, 6 hosted Swift tests,
and 6 native UI workflows per family**, all with exit code 0. The final hosted
suite was also rerun on iPad after refining the packaging test. Native result
summaries reported no failures or skips.

The new packaging regression failed before the icon was configured and passed
afterward. It reads the raw packaged plist/resources because Foundation's runtime
bundle lookup filters device-qualified iPad keys on iPhone. It decodes the real
compiled icons declared for both device families, rather than only checking the
source asset name. Existing native Files import/export/reopen tests pass with the
new **PdfCraft** folder name.

The source icon was checked as opaque 1024x1024. The built bundle's display name,
development region and copyright match the macOS template; full/numeric versions,
build commit, separate POC build number and bundled licence notices were checked.
Invalid marketing versions are rejected and prerelease/build suffixes remain in
the full version without entering the numeric marketing version.
The asset provenance/hash audit (`cargo xtask assets`) passed.
Device Hub inspection confirmed the lion icon and **PdfCraft** name on both
Simulator Home screens. The full cross-platform icon regeneration script was not
run; its macOS-to-iOS converter was run directly against the existing macOS PNG.
This remains local POC/Simulator evidence, not a signed archive or store submission.

### Shared-source maintenance validation (2026-11-16)

The metadata copies were subsequently replaced with generation from the macOS
template. An isolated fixture confirmed that changed desktop names, development
language, copyright/XML escaping, full/prerelease version and build commit flow
through automatically; desktop-only fields stay excluded and missing required
metadata fails explicitly. No production desktop source was edited for this check.
The existing test entry point now compares the built app's shared identity fields
with the desktop source to catch drift.

Both canonical device runs passed again with the same **7 Rust / 6 hosted Swift /
6 UI** counts above, exit code 0 and no native failures/skips. Rust document
semantics, the bridge API and the visible native workflows were unchanged.

## Safety and current limitations

- Local PDF processing only: no accounts, analytics, telemetry, upload, document logging,
  model downloads, or cloud dependency. A user-selected Files provider or share destination
  may itself use a network; that is an OS/provider action, not an engine upload.
- One open document per session; regular input files up to **32 MiB**, **1-2,000 pages**.
  Page dimensions must be finite and positive, with aspect ratios between 1:100 and 100:1.
- Render requests cap the edge at **2,048 pixels**, target **4 million pixels**, and
  at most **2 pixels per point**. The UI requests a 1,600-pixel edge.
  Text requests extract only one page and return at most **256 KiB of UTF-8 text**,
  cut only at a Unicode scalar boundary and explicitly marked incomplete. This bounds
  the Swift payload after extraction, not the engine's extraction memory or CPU usage.
  At most eight active export copies are retained per session, not eight lifetime
  attempts. Files completion/cancellation and Share dismissal release the corresponding
  private copy; document actions stay disabled until that cleanup finishes. Active
  copies are never recycled to make room. Rust callers must call `release_copy` after
  consumers finish, or drop the session to clean the workspace.
  A cleanup error retains the lease and keeps actions disabled; the error alert
  offers Retry Cleanup or explicit Discard and Close rather than losing the path.
- These are input/raster bounds, **not** a hard whole-process memory or CPU sandbox.
  Parser/decompression/font complexity and extreme documents can still exceed device
  budgets. There is no claim of hostile-document certification.
- Cancellation prevents queued/stale renders from publishing. It cannot interrupt an
  already-running synchronous Rust operation. Closing waits for the worker; no thread
  is forcibly killed. Hard deadlines/process isolation need separate engine work.
- Normal session destruction removes private snapshots, previews, and staged exports.
  Force-kill cleanup is left to OS temporary-storage lifecycle; no crash-recovery or
  guaranteed immediate secure erase is implemented.
- Engine errors are surfaced to the user. No catch turns a failed open/render/save into
  a successful empty document. Terminal worker failures invalidate the cached bridge
  session, including failures during the initial open, so a new import can recover.
  Ordinary document errors preserve the healthy worker and current document.
  Terminal action/render/text errors also clear the stale native document state;
  ordinary errors preserve the document and any pending cleanup lease.
- Raster pages have controls/labels for accessibility; **Read Text** provides selectable
  text and VoiceOver access in a separate sheet, not a position-aligned text overlay.
  There is no custom read-aloud player or OCR. Reading order and Unicode fidelity depend
  on the engine and the PDF's encodings. Localization, thumbnails, password UI,
  signatures, annotation editing, undo UI, document restoration, and full Acrobat
  feature parity are not claimed.
- Swift concurrency checks pass, but initial Simulator diagnostics reported a priority
  inversion warning where a Swift actor waits for the default-QoS Rust worker. Work is off
  the main actor; QoS tuning and Instruments/device memory profiling remain evaluation work.
- Full document storage and rendering fidelity come from upstream; the Swift shell does
  not silently fall back to another PDF engine.

## Contribution surface

- [crates/mobile/](../../crates/mobile/): bounded Rust facade, generated FFI, real-engine tests.
- [crates/automation/src/lib.rs](../../crates/automation/src/lib.rs): pre-open JavaScript policy builder.
- [PdfCraftMobilePOC/](PdfCraftMobilePOC/): native shell and platform integration.
- [PdfCraftMobilePOCTests/](PdfCraftMobilePOCTests/) and
  [PdfCraftMobilePOCUITests/](PdfCraftMobilePOCUITests/): hosted/interactive validation.
- [scripts/](../../scripts/): repeatable Rust, binding, project, Simulator build/run/test commands.
- Workspace membership, layer registration, lockfile, ignored generated project,
  shared identity generation and VS Code tasks support the native shell. The
  aggregate Apple driver delegates to existing build/test/packaging implementations;
  the macOS packager's optional profile flag preserves Release as its default.
  Existing desktop/web document behavior and app CI remain unchanged.

No CI, publishing, upstream endorsement, physical-device run, TestFlight, or App Store
readiness is implied. Keep follow-on features out until this slice is validated on real
iPhone/iPad hardware and reviewed for upstream fit.
