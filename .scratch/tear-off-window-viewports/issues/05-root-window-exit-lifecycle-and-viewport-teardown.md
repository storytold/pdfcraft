# 05: Root Window Exit Lifecycle & Multi-Viewport Teardown

**What to build:**
- Explicit application teardown coordination: closing the primary window initiates application shutdown by evaluating `self.first_dirty()` across all documents (both docked and detached).
- Prompts sequentially for dirty documents, cancels exit if requested, and cleanly shuts down all child viewports and worker threads when resolved without orphaned processes.
- Integration test verifying application exit with dirty detached windows.

**Blocked by:** 04-detached-child-viewport-close-and-unsaved-routing

**Status:** closed

- [x] `guard_quit` and `request_quit` in `crates/ui-egui/src/editing/close.rs` inspect both docked and detached views for dirty state.
- [x] If any document is dirty when the primary window close is requested, quit is cancelled via `egui::ViewportCommand::CancelClose` on the root viewport, and save prompts cycle through.
- [x] Once all documents are saved or discarded, `self.quit(ctx)` closes the application and all child viewports terminate cleanly.
- [x] Integration test in `crates/ui-egui/tests/it/ui.rs` verifying root window quit with floating detached windows.
