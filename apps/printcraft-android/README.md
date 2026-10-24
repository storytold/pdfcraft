# printcraft-android

PrintCraft on Android: a small `apps/` crate that hosts the existing egui UI in an
`android-activity` (GameActivity) window, plus one small seam in the shared UI crate. PrintCraft is
pure Rust — the PDF parser, interpreter, rasteriser and text extractor are its own crates — so the
only platform code here is the Java picker bridge.

## What the shell does

- **Entry point** — builds as `libmain.so`, loaded with `System.loadLibrary`; `android_main` is the
  usual `android-activity` entry.
- **File dialogs** — PrintCraft is the one Craft app whose dialogs live *inside* the shared UI crate
  (46 `rfd` call sites) instead of a host hook, and neither `rfd` nor `arboard` compiles for Android.
  `crates/ui-egui/src/fd.rs` is therefore a drop-in: on every platform `rfd` supports it re-exports
  `rfd`'s types; on Android it is a lookalike with the same builder API that asks a `Host` the app
  shell installs. The shell answers from the Storage Access Framework: an open copies the picked
  document into `<filesDir>/import/`; a save reserves a path in `<filesDir>/export/` and a worker
  publishes the finished file to the `ACTION_CREATE_DOCUMENT` destination once it stops growing.
- **Opening a file** — File ▸ Open uses the *asynchronous* dialog the UI already polls
  (`pickers::Pickers`), so the SAF answer arrives on a later frame and the app's own
  `process_picked → open_path` opens it. No dialog code was rewritten for Android.
- **Storage** — settings in `<data>/app.ron`, recovery snapshots in `<data>/recovery`, imports and
  scratch exports under the same directory.
- **Phone start-up state** — Read mode with the left tool panel closed, 40 pt hit targets, no zoom,
  plus a command palette (long-press the top 9% for 500 ms) that runs the app's own commands.
- **Self-test** — a background thread opens an embedded PDF through the normal path and renders a
  page, proving the pure-Rust PDF stack on the device.
- **Fonts** — the system CJK face is appended to egui's live font definitions (after the theme
  install, and re-applied if a language switch rebuilds the fonts).
- **`crates/ocr`** — the last-resort model-search path is gated out on Android: `env!` would bake the
  builder's home directory into the shipped `.so` (and an APK has no source tree anyway).

## Known gaps

- The ~20 *synchronous* dialogs (Attach a file, Import data, Choose a digital ID, …) answer
  "cancelled" on Android: a picker cannot answer inside the frame that asks. Making them work is a UI
  change (async follow-ups), not a platform one. The critical path — open, save, save as, export — is
  wired.
- The system clipboard is not wired; `arboard` has no Android backend.
- Folder questions answer with the app's own `exports/`; `ACTION_OPEN_DOCUMENT_TREE` is not wired.
- Printing has no backend (`pdfcraft-print` spools through `lp`), so the dialog is unreachable.
- Launcher icons are the Android Studio template's placeholder art.

## Build

```bash
cargo ndk -t arm64-v8a -o apps/printcraft-android/jniLibs build --release -p printcraft-android
cd apps/printcraft-android/android && ./gradlew assembleRelease
```

`jniLibs/` and the Gradle output are gitignored; the APK is not part of this change.
