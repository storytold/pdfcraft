# PR: Fix Preferences Scrollbar Overflowing UI Elements and OK Button (#3557)

## Summary
- **Issue Reference:** Resolves #3557
- **Crate:** `crates/ui-egui`
- **Scope:** Layout of `Dialog::Preferences` in `crates/ui-egui/src/dialogs.rs` and `crates/ui-egui/src/js_ui.rs`.

## Root Cause
Previously, `Dialog::Preferences` wrapped the entire preferences dialog body (including the OK button and 2-column theme cards) inside a single unpadded `ScrollArea::vertical()`. In egui, scrollbars are positioned along the inner right margin of the scroll area:
1. Because the column layout (`ui.columns(2)`) allocated the full available width to the two theme cards without reserving scrollbar gutter padding, the vertical scrollbar overlapped directly onto the right ("Dark theme") card.
2. Because the OK button was located at the end of the scrollable content, the scrollbar extended down past the settings content and overlapped the OK action button.

## Fix
- Separated dialog container layout into scrollable settings (`preferences_content`) and a fixed bottom action bar containing the "OK" button.
- Wrapped scroll content in an inner frame reserving 10px right margin for the scrollbar gutter, preventing scrollbars from overlapping the theme cards or controls.
- Kept the "OK" button anchored outside the scroll container so the scrollbar remains bounded within its intended settings viewport.
