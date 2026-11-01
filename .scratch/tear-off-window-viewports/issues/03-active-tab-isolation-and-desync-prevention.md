# 03: Active Tab Isolation & Desynchronization Prevention

**What to build:**
- Enforce the invariant that `self.active` references only docked tabs (`!v.detached`), shifting to an adjacent docked tab or `None` (displaying the Home/Welcome view) when tabs are detached.
- Update `close_tab` and `close_and_continue` to filter out detached tabs when selecting the next active tab, and decouple `resolve_close` so saving a detached tab does not mutate `self.active`.
- Filter out detached views from keyboard tab cycling shortcuts (Ctrl+Tab, ⌘⌥→).
- Prevent concurrent dual-rendering and dual-event processing of the same document view in both the main window and a detached viewport.
- Integration tests verifying focus transfer upon tab detachment and tab closure with detached tabs open.

**Blocked by:** 01-wasm-gating-and-sole-tab-guard

**Status:** complete

- [x] `self.active` never references an index where `views[i].detached == true`.
- [x] `detach_tab` shifts `self.active` to the nearest non-detached tab, or `None` if no docked tabs remain.
- [x] `close_tab` and `close_and_continue` in `crates/ui-egui/src/app/files.rs` and `crates/ui-egui/src/editing/close.rs` only select non-detached tabs for `self.active`.
- [x] `resolve_close` saves detached documents directly without mutating `self.active`.
- [x] Integration tests in `crates/ui-egui/tests/it/ui.rs` verify focus transitions when detaching active tabs and closing docked tabs with detached windows present.
