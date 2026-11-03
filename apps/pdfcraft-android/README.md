# PdfCraft for Android

PdfCraft on phones: the engine (`crates/engine` and everything below it) as a native library, with
a touch UI written in Jetpack Compose. It is one more frontend next to the desktop app, the CLI/MCP
server and the web build, and like the CLI it drives the engine through the automation tools
(`page_rotate`, `comment_add`, `form_fill`…) and render requests. Nothing of the egui UI is used.

Status: beta. You can open PDFs from anywhere (files, other apps, Recent), read them (continuous
or single page, Reading Mode, Night Mode, find, text selection, links, bookmarks and the table of
contents, attachments, Read Aloud), comment (notes, voice comments, highlight, underline,
strikethrough, drawing with snap to shape, text), fill forms (fields, or Fill & Sign with text,
marks and a drawn or imported signature), edit text and add text and images, organize pages, and
make new files (compress, combine, set password, create from images, text or office documents,
crop, export to Word, RTF, Excel, PowerPoint or images, scan, recognize text). The quick toolbar
can be customised. Changes are saved as you go, back into the file you opened.

The screens follow the reference phone app's layout and behaviour. The icons are Lucide (ISC,
`assets/icons`, turned into Compose path data at build time); the font is Inter; the name and the
words are PdfCraft's own.

## Layout

| Path | What |
|---|---|
| `src/lib.rs` | The `pdfcraft-android` crate: [`MobileSession`](src/lib.rs) wraps `pdfcraft_automation::Automation` for UniFFI (Kotlin now, Swift later) |
| `src/tests.rs` | Host tests of that API (`cargo test -p pdfcraft-android`) |
| `uniffi.toml` | Bindings config: the Kotlin package `io.github.dikshant363.pdfcraft.core` |
| `bindgen/` | `uniffi-bindgen`, pinned to the crate's UniFFI release; the Android build runs it |
| `android/` | The Android Studio project (Gradle, Kotlin, Compose) |

## Build and run

You need:
- Rust ≥ 1.95 with the Android targets: `rustup target add aarch64-linux-android x86_64-linux-android armv7-linux-androideabi`
- [cargo-ndk](https://github.com/bbqsrc/cargo-ndk): `cargo install cargo-ndk`
- the Android SDK with NDK 29.0.14206865 (Android Studio → SDK Manager → SDK Tools → NDK)
- JDK 17 or newer (Android Studio's bundled one is fine)
- the text-recognition models (12 MB, CC-BY-SA-4.0) are committed in `assets/models/`, so the
  build downloads nothing; `./fetch-models.sh` from this folder fetches them again (`cargo xtask
  models` in the full PdfCraft workspace). Without them the app builds and Recognize text says it
  isn't installed

Then open `apps/pdfcraft-android/android` in Android Studio and press Run, or from a terminal:

```sh
cd apps/pdfcraft-android/android
./gradlew installRelease                                # phone (arm64) and emulator (x86_64) libraries
./gradlew installRelease -Ppdfcraft.abis=arm64-v8a      # just the phone's: half the Rust build time
./gradlew installRelease -Ppdfcraft.abis=armeabi-v7a    # a 32-bit ARM phone
```

In PowerShell, quote the property: `.\gradlew.bat installRelease "-Ppdfcraft.abis=arm64-v8a"`.

The Gradle build runs `cargo ndk … build --release -p pdfcraft-android` (always the release
profile: a debug engine is too slow), then generates the Kotlin bindings from the library it built.
The first build compiles the engine for each ABI (several minutes each); after that cargo rebuilds
only what changed. Gradle finds `cargo` in `~/.cargo/bin` (or `$CARGO`).

## How it works

- **One session per process.** `PdfcraftApplication` loads the engine once on a background thread;
  `AppViewModel` holds the open document and everything around it.
- **Files.** A document opened from another app (a `content://` URI) is copied into the app's files
  and the engine works on that copy; saving writes it back over the original when the app may
  write there, and otherwise offers Save a copy. Saves are incremental, so signatures over earlier
  revisions stay valid. The app saves when it goes to the background and when the document closes,
  as the reference does. Results of the tools (compressed, combined, protected, created, scanned)
  go to Downloads through MediaStore.
- **Threads.** Every `MobileSession` call blocks, so the app calls them off the main thread. Tools
  go through one serial dispatcher. Renders hold the session lock only to look up the document's
  bytes; pages are drawn by renderers of their own, so tools and other renders don't wait.
- **The page view** (`ui/viewer/PageCanvas.kt`, `ViewerState.kt`, `DocLayout.kt`) lays the pages out
  in one column (continuous) or one per screen (single page), zooms about the pinch and scrolls with
  fling. Each page is drawn whole at the fit scale, and when zoomed in the visible part is drawn in
  512 px tiles at the zoom level's scale (half-octave steps). `PageImages` keeps an LRU of images
  within a budget and renders the newest wishes first.
- **Text.** Selection, search and Read Aloud use the engine's text layer through typed calls
  (`word_at`, `nearest_glyph`, `text_rects`, `find_in_page`, `page_plain_text`).
- **Tools** (`ui/viewer/Tools.kt`, `FillSign.kt`, `Edit.kt`, `Forms.kt`) take one-finger touches
  over the page and leave two fingers to scroll and zoom; what they make goes to the engine as
  comments (`comment_add`), Fill & Sign elements (`fill_sign_add`), form values (`form_fill`) or
  page content (`text_edit`, `page_add_text`, `page_add_image`), each one undoable.
- **Never crash.** The workspace's rules apply to this crate too (no `unwrap`/`expect`/`panic`
  outside tests, `#![forbid(unsafe_code)]`: UniFFI's generated glue builds under it). Bad input
  returns `MobileException`, and a panic that escapes a call reaches Kotlin as an
  `InternalException` instead of killing the app.

## Licences of what the app adds

- [UniFFI](https://github.com/mozilla/uniffi-rs) (MPL-2.0): the `uniffi` crate in the library, and
  the generator of the Kotlin bindings (generated at build time, not committed).
- [JNA](https://github.com/java-native-access/jna) (Apache-2.0, dual-licensed with LGPL-2.1): how the
  Kotlin bindings call the library. The app uses it under Apache-2.0.
- AndroidX, Jetpack Compose, ExifInterface and kotlinx.coroutines (Apache-2.0).
- Lucide icons (ISC), Inter (OFL-1.1) and the ocrs text-recognition models (CC-BY-SA-4.0), all
  listed in `ATTRIBUTION.toml`; the app's About ▸ Open-source licences shows them.
- The launcher icon is PdfCraft's own (`assets/app-icon/hicolor/512x512`), copied in at build time.

No Adobe code, icon, image, font or string is in the app.

## Not done yet

- Cropping a picture in Edit PDF (the engine can't yet).
- Word 97 (.doc) export, and a document language for exports.
- Recognize text reads only the Latin alphabet without accents (the one model there is).
