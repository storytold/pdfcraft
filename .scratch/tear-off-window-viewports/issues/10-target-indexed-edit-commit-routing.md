# 10: Target-Indexed Edit & Form Typing Commit Routing for Detached Viewports

**What to build:**
In-flight text box typing, form input, and queued edits originating from a detached window commit strictly against the target detached document's view index. Edit application routines accept the target view index (`apply_edit_to`, `commit_form_typing_in`, `commit_open_text_edits_in`, `process_pending_edits_for`) rather than defaulting to `self.active`. This eliminates data corruption where detached edits were applied to the main window's active document or silently dropped when all tabs were detached.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] In `crates/ui-egui/src/editing.rs`, define `apply_edit_to(&mut self, view_index: usize, edit: Edit) -> bool` and make `apply_edit` delegate via `self.active`.
- [ ] Define `commit_form_typing_in(&mut self, view_index: usize) -> bool`.
- [ ] Define `commit_open_text_edits_in(&mut self, view_index: usize) -> bool` and `commit_open_fill_text_in(&mut self, view_index: usize) -> bool`.
- [ ] Define `process_pending_edits_for(&mut self, view_index: usize) -> bool` and invoke it in `show_detached_windows` pass in `crates/ui-egui/src/app.rs`.
- [ ] In `crates/ui-egui/src/editing.rs::commit_typing_in`, pass the target view index through the commit helpers.
- [ ] Add integration test in `crates/ui-egui/tests/it/ui.rs`: with docked Tab A and detached Tab B, type in Tab B and verify edits apply to Tab B without modifying Tab A.
