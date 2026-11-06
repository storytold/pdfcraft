# 02: Tab Strip Slot-to-View Index Translation for Tab Drag-and-Drop

**What to build:**
- Accurate tab reordering when one or more tabs are detached by translating visible slot drop indices (`slot_to`) back to absolute `self.views` indices before calling `move_tab`.
- Fix the bug where dropping a tab past existing detached tabs places it at the wrong index.
- Integration test verifying that dragging a tab past a detached tab places it at the exact target slot without index corruption.

**Blocked by:** None (can start immediately).

**Status:** closed

- [x] In `crates/ui-egui/src/chrome.rs`, the drop target index `to` calculated from `drop_index(&slots, from, pos.x)` is mapped back to the underlying `app.views` index using `slots`.
- [x] Integration test in `crates/ui-egui/tests/it/ui.rs` where a tab prior to the drop target is detached, and another tab is dragged across other docked tabs, verifying that the tab is moved to the exact desired index in `self.views`.

