# Issue 07: Resolve PR #5357 Review Comments and Stabilize Viewports

**Labels:** done  
**Specification:** [`docs/designs/tear-off-window-viewports.md`](docs/designs/tear-off-window-viewports.md)  
**Parent Epic:** Issue #5348 (Tear-off document window viewports)

## Objective
Implement all code and specification fixes identified during PR #5357 review and stress-tested through the Goldfish Review Gate:
1. **Target-indexed edit routing:** Update `apply_edit_to`, `commit_form_typing_in`, `commit_open_text_edits_in`, and `process_pending_edits_for` to target the specific detached view index rather than `self.active`.
2. **Main window shortcut scoping:** Update `modal_open()` to ignore `CloseRequest::Tab` for detached views so main window shortcuts remain responsive.
3. **Detachment persistence across reload:** Copy `fresh.detached = self.detached` in `DocView::document_replaced`.
4. **Foreground layer rendering for tear-off badge:** Use `ui.ctx().layer_painter(...)` to prevent scroll-container clipping of the drag badge.
5. **Deterministic viewport disposal:** Send `egui::ViewportCommand::Close` in `close_and_continue` for closed detached tabs.
6. **WASM capability discovery:** Gate `ui.detach` and `ui.attach` command enablement with `!cfg!(target_arch = "wasm32")`.
7. **Documentation links:** Clean up local machine `file://` URLs in review notes and update specification snippets.

## Acceptance Criteria
- [x] Detached tabs retain detached window state after replacing document on disk.
- [x] Keyboard shortcuts in main window remain operational while save prompt is displayed inside a detached window.
- [x] Text editing and form filling in detached viewports commit against the detached document even when another tab or no tab is docked in the main window.
- [x] Drag-to-detach preview badge remains visible when dragged outside the tab strip boundaries.
- [x] Closing a dirty detached tab issues `ViewportCommand::Close` upon Save or Don't Save completion.
- [x] Headless discovery reports `ui.detach` as disabled on WASM builds.
- [x] All integration tests in `crates/ui-egui/tests/it/ui.rs` and `control.rs` pass cleanly.
