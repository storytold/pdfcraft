# 08: Detachment State Persistence on Document Replacement & Reload

**What to build:**
When a detached document is reloaded or replaced from disk (for example, after being modified externally), the view reinitialization preserves `detached = true`. The separate native OS window stays open and intact instead of vanishing and collapsing back into the docked tab strip.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] In `crates/ui-egui/src/canvas.rs::DocView::document_replaced`, copy `fresh.detached = self.detached`.
- [ ] Add an integration test in `crates/ui-egui/tests/it/ui.rs` verifying that calling `document_replaced` on a detached view keeps `detached == true` and does not disturb `self.active`.
