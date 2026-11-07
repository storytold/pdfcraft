# 11: Main Window Keyboard Shortcuts Isolation During Detached Save Prompts

**What to build:**
When a user attempts to close a detached window with unsaved edits, `modal_open()` and canvas input blocking ignore the tab-scoped save prompt. Keyboard shortcuts and document interaction in the main application window remain fully operational rather than being globally deadlocked.

**Blocked by:** 10-target-indexed-edit-commit-routing

**Status:** ready-for-agent

- [ ] In `crates/ui-egui/src/app.rs::modal_open()`, check `self.close_request` but ignore `CloseRequest::Tab(id)` where `id` corresponds to a detached view (`v.detached == true`).
- [ ] In `crates/ui-egui/src/app.rs::logic()`, scope `view.objects.set_input_blocked` to the specific view displaying the save prompt rather than blocking all views.
- [ ] In `crates/ui-egui/src/app/keys.rs::shortcuts()`, ensure keyboard shortcut execution proceeds for the main window when `self.close_request` belongs to a detached tab.
- [ ] Add integration test in `crates/ui-egui/tests/it/ui.rs`: while a detached window displays a modal save prompt, keyboard shortcuts (zoom, page navigation, view toggle) in the main window execute successfully.
