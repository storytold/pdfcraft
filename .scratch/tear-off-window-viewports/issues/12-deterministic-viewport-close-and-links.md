# 12: Deterministic Viewport Close Commands on Dirty Save/Discard & Link Cleanups

**What to build:**
1. In `close_and_continue` (`crates/ui-egui/src/editing/close.rs`), explicitly issue `egui::ViewportCommand::Close` to the detached child viewport ID upon completion of Save (both synchronous and asynchronous file-picker saves) and Don't Save.
2. In `docs/reviews/2026-11-01_feat-tear-off-window-viewports-issue-5348_review.md`, replace machine-local `file://` URLs with relative repository paths.
3. In `.scratch/tear-off-window-viewports/issues/06-headless-ui-control-channel-and-parity.md`, clarify that multiple windows is a GUI feature with evidence provided by UI and control-channel tests.

**Blocked by:** 11-shortcut-isolation-during-detached-prompt

**Status:** ready-for-agent

- [ ] In `crates/ui-egui/src/editing/close.rs::close_and_continue`, dispatch `ctx.send_viewport_cmd_to(ViewportId::from_hash_of(("detached-doc", id)), ViewportCommand::Close)` when closing a detached tab.
- [ ] Add assertions in `crates/ui-egui/tests/it/ui.rs` that `ViewportCommand::Close` is emitted in viewport output for both Save and Don't Save paths of dirty detached close.
- [ ] Replace all machine-local `file:///media/saifallah/...` URLs in `docs/reviews/2026-11-01_feat-tear-off-window-viewports-issue-5348_review.md` with repository-relative paths.
- [ ] Clarify parity notes in `06-headless-ui-control-channel-and-parity.md`.
