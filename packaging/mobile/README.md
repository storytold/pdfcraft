# PdfCraft for Android and iPad/iPhone

The mobile app embeds the existing Rust/egui WASM editor in a Tauri 2 host. It bundles the complete frontend and all 35 existing UI languages; no website or runtime CDN is loaded. Interface language follows the device locale, with English fallback, and remains selectable in the shared editor. Builds require the pinned craft-fonts checkout used by desktop releases. Tablet portrait, landscape and iPad Split View share the same editor; the web canvas reserves the device safe-area insets.

## Dependency-policy review required

Tauri's mandatory Android WebView HTML parser and cross-platform directory helper include five MPL-2.0 crates. At the contributor's explicit request, this PR proposes exact-version exceptions in the standalone mobile workspace, documented in [DEPENDENCIES.md](DEPENDENCIES.md). The desktop policy is unchanged. Maintainer acceptance of this mobile-only proposal is required before merging or an official upstream release; it is not a general permission for copyleft dependencies. CI still enforces the root policy, with only these five scoped exceptions.

## Build

Install Rust 1.95.0, `trunk 0.21.14` and `tauri-cli 2.12.2` with Cargo (`--locked`). Android needs Java 17, Android SDK platform 37.0 (`platforms;android-37.0`), Build Tools 36.0.0 and NDK 29.0.14206865; iOS/iPadOS 16+ needs macOS/Xcode and XcodeGen; the bundled WebAssembly frontend uses the modern WebKit CSP support. The native host is an independent Cargo workspace, so mobile dependencies do not affect desktop releases.

```sh
cargo install trunk --version 0.21.14 --locked
cargo install tauri-cli --version 2.12.2 --locked
rustup target add wasm32-unknown-unknown
# Set CRAFT_FONTS_DIR to the pinned craft-fonts checkout from mobile.yml, and CRAFT_I18N_DIR
# to a storytold/craft-i18n checkout at the commit in craft-i18n.rev (the interface catalogs).
cargo xtask mobile frontend
cargo xtask mobile prepare android
cargo xtask mobile android
# On macOS instead:
cargo xtask mobile prepare ios
cargo xtask mobile ios-simulator
```

The generated Gradle/Xcode projects are ignored, recreated using the pinned CLI and populated with attributed PdfCraft icons. The iOS privacy manifest declares only the file-timestamp API used for user-selected document access. No analytics or tracking is included. `.github/workflows/mobile.yml` builds debug ARM64/x86_64 APKs and an unsigned ARM64 iOS simulator app on hosted runners with time limits. A simulator bundle is **not** installable on a physical iPad or iPhone.

Fork distributions must use `cargo xtask mobile frontend-preview`: it removes the protected upstream marks from the built UI, identifies it as an independent preview, and restores the original source files after building (including build failure). Run it in a disposable checkout without concurrent builds; termination of the packaging process itself can interrupt restoration. CI automatically selects this mode outside `storytold/pdfcraft`. The shared UI sources in the contribution remain unchanged by this packaging policy. See [the brand license](../../docs/brand/LICENSE-brand.txt).

The frontend build also collects the project, Lucide, embedded-font, colour-profile and CMap license texts from their checked-in or Cargo-locked sources, including the selected craft-fonts web faces. It appends the license/copyright/notice files, authors and source declarations from both locked Cargo graphs, including the native host and build dependencies. `THIRD-PARTY-NOTICES.txt` travels in the frontend artifact and is bundled as a native resource in both apps. Missing required notices fail packaging. `cargo xtask mobile notices` can regenerate this file independently for host testing.

Android additionally applies `android-notices.gradle` to the generated application project. For each built variant, Gradle resolves its actual external runtime AARs/JARs and Maven POMs, including transitive AndroidX/Material dependencies. Before merging APK assets, the task retains complete original LICENSE, NOTICE, COPYRIGHT and COPYING entries (also inside nested JARs) without changing their bytes. `assets/ANDROID-THIRD-PARTY-NOTICES.txt` records coordinates and SHA-256 values; `assets/pdfcraft-android-licenses/` contains the retained texts, exact POMs, Apache-2.0 text and the pinned Material notice. These files supplement the Cargo/frontend notices; they do not relicense dependencies. Missing artifacts/POMs, invalid archives or exceeded bounds fail the build. The adapter only resolves metadata and invokes the Rust collector; `cargo test -p xtask mobile::android_notices` covers nested notices, byte preservation, invalid archives and size limits. Native SDK CI must verify the resulting APK contents before publication.

For physical iOS devices, set `APPLE_DEVELOPMENT_TEAM` and configure the signing certificate/provisioning profile in Xcode, then run `cargo tauri ios build --target aarch64` from `packaging/mobile/app`. Android production signing is configured in the generated Gradle project using the official Tauri guide. Signing keys, store accounts and store publishing are not part of this contribution. The application identifier is `org.pdfcraft.mobile`; a distributor must choose an identifier it can provision before releasing.

## Documents and export guarantees

Open invokes the system PDF picker (including Android document providers). `file://` imports keep their decoded name; the current Rust provider API exposes Android content as a URI without a display name, so those imports use `document.pdf` until a provider-name adapter is available. PDF file associations accept OS Open/Share intents. Native read access is restricted to picker/OS-issued one-use tickets; the webview cannot submit arbitrary filesystem paths, and it has no filesystem plugin capability. Imports and PDF exports are limited to 64 MiB per document and eight queued imports. The current PDF Save/Save As opens a native destination picker; it does not silently overwrite the imported URI. A document is marked saved only after the provider confirms write and sync. Cancelled/failed writes and newer edits keep their unsaved state.

Providers do not guarantee atomic replacement. Before writing, the host retains a private, durable recovery copy. If writing fails, that PDF reopens on the next app launch, under its generated `export-*.pdf` name; save it to recover it. Incomplete `.writing-*` files are never published as recovery PDFs and are cleaned on next launch. A native opaque identity binds each recovery copy to its opened document, independently of its filename. Unix directory entries are synced before a document provider is opened. Up to eight such copies (512 MiB maximum payload) are retained. Recovered tabs remain exportable at that limit. Removing/uninstalling the app removes its private recovery storage. These are recovery copies of attempted saves, **not** continuous autosaves; save before switching away from the app because a mobile OS may terminate a background webview.

The existing browser-download export paths (non-PDF conversion, split/export actions, etc.) explicitly report that they require the browser/desktop app. They never report a native save they cannot confirm, particularly before delete-after-extract operations. PDF editing, annotations, form filling and saving use the real shared engine. OCR (including optional historical models) is unavailable in this WASM-based host because recognition currently discovers models through native filesystem paths. Hot Folder/service processing is desktop-only. Other desktop-only engine capabilities retain the web build's limits. This first mobile host does not claim native scanner/camera integration, background processing, pressure-sensitive ink or palm rejection.

## Validation

Native adapter tests cover path/name/size bounds. Shared UI tests simulate native success, cancellation, failure and edits made while saving; an older exported snapshot cannot close or replace a newer document. Packaging tests ensure every interface language in `i18n.rs` is advertised to iOS and that PDF associations/safe areas survive changes. CI additionally builds the actual Android and iOS targets. Device-level PDF picker/provider, suspend/resume, handwriting and highlighting checks on physical tablets remain required before a stable release; compilation alone is not that verification.

Primary implementation references: [Tauri mobile file associations](https://v2.tauri.app/learn/mobile-file-associations/), [dialog plugin](https://v2.tauri.app/plugin/dialog/), [filesystem plugin](https://v2.tauri.app/plugin/file-system/), [Android distribution](https://v2.tauri.app/distribute/google-play/) and [iOS distribution](https://v2.tauri.app/distribute/app-store/).
