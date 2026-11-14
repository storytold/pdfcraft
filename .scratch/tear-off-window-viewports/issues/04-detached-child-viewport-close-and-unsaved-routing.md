# 04: Detached Child Viewport Close Interception & Unsaved-Changes Routing

**What to build:**
- Clean closure handling for detached windows: clean tabs close immediately and unregister their viewports.
- For detached documents with unsaved changes, intercept `close_requested()` using `egui::ViewportCommand::CancelClose` to abort premature OS destruction, route the save confirmation dialog directly inside the detached child window, and respect Save / Don't Save / Cancel choices.
- Integration tests verifying clean close, dirty close interception, cancel retention, and save-then-close on a detached window.

**Blocked by:** 03-active-tab-isolation-and-desync-prevention

**Status:** closed

- [x] Detached windows check `child_ui.input(|i| i.viewport().close_requested())`.
- [x] If the detached view has unsaved changes, issue `egui::ViewportCommand::CancelClose` to `viewport_id` and display the save prompt within the child viewport.
- [x] Choosing "Don't Save" discards changes and closes the child window.
- [x] Choosing "Cancel" dismisses the prompt and leaves the child window open with unsaved changes preserved.
- [x] Choosing "Save" saves the document and closes the window once the write completes.
- [x] Clean detached windows close immediately.
- [x] Integration tests in `crates/ui-egui/tests/it/ui.rs` verify all close paths for detached viewports.
