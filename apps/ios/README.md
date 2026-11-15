# PdfCraft iOS Proof of Concept

This branch contains an experimental iOS/iPadOS integration of the open-source PdfCraft engine.
The goal is to evaluate a native mobile architecture using the existing PdfCraft Rust core with a Swift/SwiftUI interface.
This project is not an official ArtCraft mobile release and does not imply endorsement by ArtCraft.
The implementation is intended for technical evaluation and potential upstream contribution.

Blackout Secure sponsors this fork experiment on `blackout-secure-ios-poc`.
The development app is **PdfCraftMobilePOC**, bundle ID `com.blackoutsecure.pdfcraft.poc`.
This is not a production release or an Adobe application, and contains no copied Adobe
code, graphics, fonts, or UI assets.

## Fork-only language exception

The fork owner explicitly approved a small Swift/SwiftUI Apple-platform shell, despite the
upstream Rust-only native-app policy. This exception is limited to this experiment.
It is **not** upstream approval and does not change the synchronized house rules in
[AGENTS.md](../../AGENTS.md). Upstream adoption requires maintainer agreement.

PDF parsing, rendering, page rotation, validation, serialization, resource limits, and
document ownership stay in Rust. Swift is limited to native views, accessibility labels,
Files access/coordination, presentation state, export/share sheets, and the typed bridge.
There is no Android target, web wrapper, WebView, Electron, or separate PDF implementation.

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
The facade wraps the existing `doc_open`, `page_render`, `page_rotate`, `doc_save`, and
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

# Rust tests, then hosted Swift bridge tests, then native UI workflow tests:
bash scripts/test-ios.sh <simulator-uuid>

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
Build scripts must run before building the generated Xcode project.
Intel Simulator and XCFramework distribution are not implemented.
Release configuration currently links the same development Rust archive as Debug;
it is not a release/signing/archive pipeline.

Use SweetPad to select the generated project, scheme, destination, and debugger.
Only open the Xcode IDE for account/signing/provisioning or Apple-specific inspection
that cannot reasonably be done through VS Code and CLI tools.
The run script boots the selected Simulator and installs/launches the app; it does not
keep a terminal watcher running. Stop the app with
`xcrun simctl terminate <simulator-uuid> com.blackoutsecure.pdfcraft.poc`, and shut down
an otherwise unused Simulator with `xcrun simctl shutdown <simulator-uuid>`.

## First vertical slice

1. **Open PDF** uses the native Files picker. Security-scoped, coordinated access stays
   active until Rust has made a bounded private snapshot; the original is never edited.
2. View locally rendered pages. Pages are loaded lazily, images are released on
   disappearance, and pinch/Zoom controls change their displayed size (maximum 3x).
   On iPad, the sidebar selects pages. Tap a page label to choose the page to rotate.
3. **Rotate** applies a clockwise 90-degree engine edit to the selected page and rerenders.
4. **Save a Copy** asks Rust to validate and atomically stage a new PDF, then opens the
   native Files export picker. Cancelling export retains the UI's unsaved-change warning.
   The engine's staged-copy dirty state and successful external export are distinct.
5. Close and reopen the exported copy through Files; rotation must persist.
6. **Share** passes a current, validated PDF copy to the native activity sheet. It does
   not require a separate Files export first and does not dismiss an unsaved-change warning.

Hosted Swift tests generate an original rectangle-only PDF in code. They also stage
`PdfCraft iOS UI Fixture.pdf` in the test app's `Documents/PdfCraft UI Workflow Test/`
folder for the subsequent UI test. That test-owned directory's previous `copy-2.pdf`
export is removed before each complete run; do not put personal documents there.
No binary fixture, font, or image asset is committed. UI tests use an English Simulator,
test real Files import/export, check portrait-to-landscape rotation and reopening, and
present the native share sheet. Run both test targets in order with the script.
The app's Documents folder is visible in Files; private working copies/previews are not
stored there.

Run one Simulator at a time; the scripts share generated bindings and build output.
Native UI tests allow up to five minutes per test, including bounded waits for Files
provider startup. The test adapts to iPhone sheet dismissal and iPad's native Cancel
control rather than replacing either platform's Files UI.

## Verified locally

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

## Safety and current limitations

- Local PDF processing only: no accounts, analytics, telemetry, upload, document logging,
  model downloads, or cloud dependency. A user-selected Files provider or share destination
  may itself use a network; that is an OS/provider action, not an engine upload.
- One open document per session; regular input files up to **32 MiB**, **1-2,000 pages**.
  Page dimensions must be finite and positive, with aspect ratios between 1:100 and 100:1.
- Render requests cap the edge at **2,048 pixels**, target **4 million pixels**, and
  at most **2 pixels per point**. The UI requests a 1,600-pixel edge.
  At most eight staged export copies are retained per session; close/reopen starts a new
  session. Repeated shares reuse a current copy.
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
  a successful empty document. A panic-disconnected session must be closed/reopened.
- Raster pages have controls/labels for accessibility but no selectable or spoken PDF
  text layer. Localization, thumbnails, password UI, scanning/OCR, signatures, annotation
  editing, undo UI, document restoration, and full Acrobat feature parity are not claimed.
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
- Workspace membership, layer registration, lockfile, ignored generated project, and VS Code tasks
  are the only supporting integration changes. Existing desktop/web entry points are unchanged.

No CI, publishing, upstream endorsement, physical-device run, TestFlight, or App Store
readiness is implied. Keep follow-on features out until this slice is validated on real
iPhone/iPad hardware and reviewed for upstream fit.
