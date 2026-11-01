# PdfCraft for iOS

A reader: open a PDF, view its pages, page through it, pinch to zoom, find text and jump through
the outline. No editing yet — that's `pdfcraft-engine`'s surface, out of scope for this crate
until a later milestone (see `crates/mobile-ffi/src/lib.rs`).

This directory is a Swift package (`PdfCraftKit`), not an app by itself — SwiftUI's view tree
(`PdfCraftRootView`) lives here, but an `.xcodeproj` app target is what actually runs on a device
or in the Simulator.

## Build

1. From the repo root (on macOS, with Xcode installed): `cargo xtask mobile ios`. This
   cross-compiles `pdfcraft-mobile-ffi` for device and Simulator, and writes
   `Frameworks/pdfcraft_mobile_ffiFFI.xcframework` and `Sources/PdfCraftFFI/` here — both
   git-ignored, regenerated after any change to `crates/mobile-ffi/src/lib.rs`.
2. In Xcode: File ▸ New ▸ Project ▸ App (SwiftUI, Swift), anywhere outside this directory. Add
   this directory as a local Swift package dependency (File ▸ Add Package Dependencies ▸ Add
   Local), link `PdfCraftKit` against the app target, and make the app's `@main App`'s body
   `PdfCraftRootView()`.

Nothing here has been built or run — this package was written and read through for Swift syntax
and the UniFFI Swift bindings' exact method signatures (verified by generating real bindings from
`crates/mobile-ffi` on this machine and reading them), but never compiled: there is no Xcode on
this (Linux) machine. Steps 1 and 2 are unverified; do them on a Mac, and expect to fix something.
