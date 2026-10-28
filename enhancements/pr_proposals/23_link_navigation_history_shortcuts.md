# PR Proposal 23: Link Navigation History Shortcuts (Alt+Left / Alt+Right)

**Issue Reference:** #3632 (also addresses ROADMAP M3 candidates)
**Crates Affected:** `crates/ui-egui`

## Summary
In Adobe Acrobat and web browsers, users navigating hyperlinks and bookmarks expect `Alt + Left Arrow` to jump back to the previous view and `Alt + Right Arrow` to step forward. Prior to this fix, only `Ctrl/Cmd + [` and `Ctrl/Cmd + ]` were wired to `view.view_history(false / true)`.

## Implementation Details
- In `crates/ui-egui/src/canvas.rs`, added modifier and key detection for `ctx.input(|i| i.modifiers.alt && i.key_pressed(Key::ArrowLeft))` and `ArrowRight`.
- Connected these directly to `view.view_history(false)` and `view.view_history(true)`, providing intuitive navigation on Windows, Linux, and macOS without disturbing existing bracket shortcuts.
