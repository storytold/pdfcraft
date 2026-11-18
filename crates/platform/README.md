# pdfcraft-platform

Layer L7 holds operating-system services that both the desktop app (`pdfcraft-ui-egui`) and the automation tools (`pdfcraft-automation`) need. Those two crates sit on the same layer and can't depend on each other, so a helper they share lives here instead of being copied into each (`xtask/src/layers.rs` allows both edges). It has no UI dependencies; macOS filesystem calls use the safe `rustix` wrapper.
