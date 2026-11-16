# Apple platform reuse, POCs and distribution readiness

Review date: 2026-11-16. **Upstream proof-of-concept proposal, not suite policy or
Apple certification.** The current desktop package is **not Mac App Store ready**.
The mobile app is not ready for TestFlight/App Review merely because it runs in
Simulator or can be archived without signing.

## Code ownership: reuse before extraction

| Responsibility | Canonical owner | Reference evidence / next step |
| --- | --- | --- |
| PDF parsing, rendering, permissions, edits, text, save fidelity | Existing Rust engine/automation and their existing `craft-libs` dependencies | [automation](../crates/automation/README.md), [workspace pins](../Cargo.toml); no second Swift PDF model |
| Mobile limits/session adapter and supported operations | App-local [Rust adapter](../crates/mobile/src/lib.rs) | Calls existing automation; do not move PDF commands into a generic host |
| OS Files scopes, coordination, share/export and native views | [iOS shell](../apps/ios/README.md) | Native iPad sidebar/iPhone sheets; one supported-tool list; no full desktop catalogue copy |
| App-neutral formats/protocols/native host primitives | `craft-libs` | [existing host proposal](https://github.com/storytold/craft-libs/pull/632), [boundary evidence](https://github.com/storytold/craft-libs/pull/1909), [inventory](https://github.com/storytold/craft-libs/issues/115) |
| Shared runner/toolchain/device/artifact orchestration | `craft-common-actions` | [existing issue](https://github.com/storytold/craft-common-actions/issues/259), [proposed POC](https://github.com/storytold/craft-common-actions/pull/260) |
| App product names, capabilities, archive commands and domain tests | This app | [aggregate driver](../scripts/build-apple.sh) delegates to existing build/test/packaging scripts |
| Suite policy and exception approval | `craft-contrib` | [RFC-0039](https://github.com/storytold/craft-contrib/pull/60), [shared-library-first RFC](https://github.com/storytold/craft-contrib/pull/62), [existing-crate migration RFC](https://github.com/storytold/craft-contrib/pull/56) |

The shared-library-first and migration RFCs already exist; do not propose duplicate
general rules. The separately proposed Apple policies address a real missing
Swift-host exception and distinct Apple distribution/readiness gates.
They are published together for review in
[storytold/craft-contrib#63](https://github.com/storytold/craft-contrib/pull/63)
(provisional RFC-0041/0042), not adopted policy.
The app implementation is submitted in
[storytold/pdfcraft#7549](https://github.com/storytold/pdfcraft/pull/7549).
The [mobile guide](../apps/ios/README.md#relationship-to-the-suite-proposal)
compares the open Tauri/egui alternatives and the relevant closed mobile proposals;
none is silently superseded or described as accepted.

### Do not duplicate the other host proposal

`craft-mobile` is unmerged. Its author's later Rust-described native panel kit is
reported in the RFC discussion but was not pushed/reviewed as part of this audit.
Compare that implementation when available before building another Swift framework.
The current PdfCraft raster/automation bridge and the proposed egui/Metal host are
different architectures, not interchangeable proof.

The `craft-libs` evidence POC exercises existing JSON-RPC helpers with two synthetic
app adapters. It does not install JSON-RPC into this local FFI, open a socket, move
PDF business logic or establish production multi-app native-host compatibility.
No new runtime crate or unapproved shared-host dependency is added here.

## One local build/test/package entry point

```sh
bash scripts/build-apple.sh build all Debug
CRAFT_IPHONE_SIMULATOR_UDID=<iphone-uuid> \
CRAFT_IPAD_SIMULATOR_UDID=<ipad-uuid> \
  bash scripts/build-apple.sh test all Debug
bash scripts/build-apple.sh package all Debug
```

Replace the UUID placeholders with an available iPhone and iPad from
`xcrun simctl list devices available`. Do not reuse one device for both families.
Use `bash scripts/build-apple.sh --help` for the positional interface:
`<build|test|package> <all|macos|ios-simulator|ios-device> <Debug|Release>`.

- `build all` builds the actual desktop app/CLI, Simulator app and iOS-device app.
  The iOS POC currently requires an Apple Silicon Mac with installed Xcode SDKs,
  XcodeGen 2.46.0 and the Rust iOS target libraries. Desktop-only builds support the
  host architecture; this does not certify universal desktop or Intel Simulator.
- `test all` runs the 32 desktop-platform unit tests, isolated build/metadata
  regressions, then the existing full
  [iOS test command](../scripts/test-ios.sh) sequentially on each selected family.
  It is an Apple POC test set, not every test in this large workspace. Native tests
  currently require Debug. An unsigned device build is not physical-device testing.
  Budget up to 20 minutes for the aggregate native suites on a slower local host;
  a 10-minute aggregate deadline was insufficient in this review.
- `package all` reuses the [macOS packager](../packaging/macos/package.sh), produces
  a Simulator ZIP, and creates a real device `.xcarchive`. The macOS packager's new
  `--profile debug|release` defaults to `release`, preserving existing callers.
  Package names/layout are retained; Debug POC products are isolated by directory.
- Release selects Cargo `--release` and Xcode Release. The former
  [Release configuration](../apps/ios/Config/Release.xcconfig) incorrectly inherited
  the Debug Rust library path; it now explicitly selects `release`. Generated
  bindings come from the corresponding Cargo invocation, not a copied source file.
- Current native generators share output. Do not run these commands or device
  suites concurrently against one checkout.

The driver never imports certificates, uploads, enrolls an account, invites testers
or submits a review. Shared CI can invoke the same reviewed script rather than
reimplement its app commands. App workflow files are deliberately unchanged.

### Actual unsigned products

Under `target/apple/<Debug|Release>/`:

| Directory | Products | Meaning |
| --- | --- | --- |
| `macos` | `PdfCraft.app`, `PdfCraft.app.zip`, existing DMG and CLI ZIP | Ad-hoc signed locally, not Apple Developer ID signed/notarized or Store packages |
| `ios-simulator` | `PdfCraftMobilePOC.app.zip` | Simulator only; not an iPhone/iPad installation package |
| `ios-device` | `PdfCraftMobilePOC.xcarchive` | Unsigned device archive; not an installable `.ipa` or TestFlight-eligible binary |

The native device archive is checked for `iPhoneOS`, not a Simulator platform.
The copied desktop bundle is signature-verified. Apple Silicon needs local code
signatures, so “unsigned” here means **no Apple distribution identity**, not that
every Mach-O signature byte is absent. Do not tell users to disable Gatekeeper.
An optimized but unsigned archive still needs authorized signing/export and review.

## Apple requirements and actual readiness

Sources were reviewed on 2026-11-16, not inferred from a green build. Apple alone
decides acceptance. These are engineering findings, not legal certification.

| Requirement | Finding | Needed before claiming readiness |
| --- | --- | --- |
| Mac App Sandbox, guideline 2.4.5(i) | **Blocked**: [desktop entitlements](../packaging/macos/entitlements.plist) explicitly describe an unsandboxed path-based app | Separate Store configuration, minimal entitlements, container/settings migration, scoped Files/bookmarks and revocation tests |
| Mac Store installation/update, 2.4.5(ii)/(iv)/(vii) | **Blocked**: current DMG/separate CLI and [desktop updater](../crates/ui-egui/src/platform/updates.rs) are a direct-download lane | Self-contained Xcode submission; Store-only updates; audit/gate helpers, downloaded code/resources and external converters/plugins |
| iOS native Release/Rust target | **Validated unsigned POC**: matching Cargo/Xcode Release built and archived; actual `iPhoneOS`, arm64, device families `1,2` checked | Signing/export, physical-device behavior and Store gates still separate |
| Universal iPhone/iPad | Declared `1,2`; compiled icons and native workflows covered on both families | Physical phone/tablet, minimum supported/current OS, memory/suspend/provider coverage remain |
| Privacy/required-reason API | **Blocked for submission**: shell uses `@AppStorage` preferences; no privacy manifest is currently bundled | Audit app and transitive SDK API use; declare accurate reasons and disclosures, including app-local UserDefaults use; do not guess a universal “no data collected” answer |
| Functional completeness/metadata, 2.1/2.3/4 | A narrow working POC; [unsupported controls](../apps/ios/PUBLISHING.md) stay hidden | Accurate feature descriptions, original screenshots, accessibility and lifecycle evidence; disclaimer does not bypass completeness |
| Beta distribution, 2.2 | **Public Store submission blocked for this evaluation build** | Use authorized TestFlight for eligible pre-release testing; mature the app for public distribution rather than merely renaming the POC |
| Signing/team/store identity | Unregistered upstream POC identifier, build counter `1`, no authorized distribution proof | Owner-created production identity, monotonic build number, matching capabilities/profiles, archive/export validation |
| Legal/privacy/export compliance | Licences and icon provenance are tracked; full Store audit unverified | Public privacy/support URLs, age/trader/legal answers, SDK manifests, authorized branding and cryptography/export-compliance assessment |

Do not “fix” these blockers by enabling sandboxing blindly or disabling desktop
features in every channel. Preserve the existing Developer ID/DMG behavior.
Track the separate Mac Store work in
[storytold/pdfcraft#7540](https://github.com/storytold/pdfcraft/issues/7540).

## Proposed user-visible differences

iOS follows desktop task order and terminology with native Files, touch targets,
sharing/text selection, an iPad sidebar and a compact iPhone tools sheet. Unsupported
desktop Edit/Convert/E-Sign controls are not fake working tabs. Rust still owns
document changes; no new renderer or undo stack is introduced.

A future Mac Store channel will need user-granted document access and revocable
bookmarks, container-safe settings/recovery and Store-managed updates. Some
CLI/helper/plugin workflows may need a separate direct-download edition. Report
those differences in capability discovery and Store metadata before publication;
do not silently remove them from existing desktop releases.

## Account, private flighting and public release

See the [publishing guide](../apps/ios/PUBLISHING.md) and
[shared proposal](https://github.com/storytold/craft-common-actions/pull/260) for
credential names/locations and protected-workflow hand-offs. No private material
belongs in this repository or shared CI source.

Recommended order: unsigned proof -> accepted architecture/shared CI -> readiness
fixes -> authorized signed pilot -> processed Apple build ID -> private internal
TestFlight -> optional invitation-only external beta/Beta App Review -> separately
approved production App Review -> manual initial public release.

macOS can also use TestFlight once its Store-compatible build is ready. Developer
ID notarization is a different channel. Recheck supported Xcode/SDKs at release:
the official current page requires iOS/iPadOS uploads built with Xcode 26+/SDK 26+
and an iOS deployment target of at least 13. This POC's compile-time minimum 17 and
local Xcode 27.2 do not prove hosted availability or minimum-OS runtime coverage.

## Validation evidence

The real aggregate Debug build and all three unsigned packaging paths completed
successfully. They produced actual desktop `.app`/DMG/ZIP, Simulator ZIP and
device `.xcarchive`, not mock artifacts. Isolated shell regressions cover profile
selection, build failure propagation, target checks, forced non-distribution
signing, packaging paths and sequential device dispatch, including paths with spaces.
Shared workflow fixtures test two synthetic consumers separately from native runs.
The unchanged shared workflow's actual-artifact validators also passed for all
three real Debug package scopes (exit 0 each); structural compatibility does not
establish signing, hosted compilation or Store acceptance.

The aggregate native tests passed with a corrected 1200-second deadline: 32
desktop-platform tests, seven Rust adapter tests per device iteration, and six
hosted Swift plus six native UI tests on each iPhone 18 Pro and iPad Pro 11-inch
(M5), iOS 27. All four XCTest result bundles report zero failures and zero skips.
The first 600-second aggregate attempt timed out during the iPad UI suite (exit
124); it was not treated as a pass. No tests were removed to make the retry pass.

The real optimized device command `bash scripts/build-apple.sh package ios-device
Release` also completed with exit 0. Xcode's resolved Release settings select
`aarch64-apple-ios/release`, including quoted custom paths with spaces. A further
six hosted tests passed using an actual library path containing spaces.
The first cold Release invocation reached `ARCHIVE SUCCEEDED` but its driver
exited 2 after being edited while Bash was reading it. The complete command was
rerun successfully against frozen script contents; the earlier exit was not hidden.
Release macOS/Simulator products and hosted GitHub Apple execution were not run.

`cargo fmt --check`, the focused `cargo clippy --locked -p pdfcraft-mobile
--features swift --all-targets --no-deps -- -D warnings`, `cargo xtask layers`,
`cargo xtask assets` and `cargo xtask wasm` passed (33 wasm crates).
The broader dependency-linting Clippy command remains blocked by pre-existing
`clippy::collapsible_match` in [unchanged XFDF source](../crates/xfdf/src/xfdf.rs);
no unrelated source or lint suppression was added. Existing vendor/wasm warnings
also remain. This is not a claim that the full workspace CI is green.

Do not extrapolate
physical-device, hosted-runner, VoiceOver speech or Apple acceptance from them.
Builds before the final source commit carried baseline HEAD plus local changes;
they were not immutable release artifacts.

## Official sources

- [App Review Guidelines](https://developer.apple.com/app-store/review/guidelines/).
- [App Sandbox](https://developer.apple.com/documentation/security/app-sandbox)
  and [sandboxed Files](https://developer.apple.com/documentation/security/accessing-files-from-the-macos-app-sandbox).
- [Upload builds](https://developer.apple.com/help/app-store-connect/manage-builds/upload-builds/)
  and [upcoming/current requirements](https://developer.apple.com/news/upcoming-requirements/).
- [TestFlight](https://developer.apple.com/help/app-store-connect/test-a-beta-version/testflight-overview/).
- [Required-reason APIs](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api)
  and [SDK requirements](https://developer.apple.com/support/third-party-SDK-requirements/).
