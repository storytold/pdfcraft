# Specification: Tear-Off Document Window Viewports (Issue #5348)

## Problem Statement

When working with multiple PDF documents—such as comparing contract revisions, cross-referencing technical documentation, or organizing and rearranging pages across different files—users cannot view more than one document simultaneously on screen. Document tabs remain locked inside a single tab strip within the primary application window. Attempting to drag a tab outside the strip or into open screen space does nothing, preventing users from positioning documents side-by-side or distributing them across multiple physical monitors.

Furthermore, multi-window desktop architectures introduce critical lifecycle and data-integrity edge cases:
- Closing a docked document while other documents are detached risks tab selection desynchronization, potentially causing the same document to render concurrently in both the primary window and a detached window, triggering dual event conflicts.
- Closing a detached window with unsaved changes risks premature operating system (OS) window destruction if the destroy event is not intercepted; additionally, routing modal save prompts to the main window disconnects prompts from the user's focus and corrupts active tab state.
- In-flight text or form edits initiated within a detached window risk being committed against whichever document happens to be active in the main window—or failing silently if no docked document is active.
- Dragging a tab outside the tab strip renders a tear-off badge that can be clipped away by container scroll boundaries when the pointer moves outside the tab strip rectangle.
- Reloading a detached document from disk risks resetting its detachment state, inadvertently snapping the window shut and returning the tab to the docked strip.
- A save confirmation prompt open inside a detached window risks globally locking out keyboard shortcuts and mouse input across the main application window.
- Drag-and-drop tab reordering within the tab strip can drift whenever detached tabs precede the drop target due to mismatched index spaces between visible slots and underlying document views.
- Attempting to invoke multi-viewport actions on WebAssembly (WASM) targets breaks application execution because multi-viewport functionality is unsupported by web canvas runtimes.

## Solution

PdfCraft will support tearing off document tabs into independent native desktop window viewports and cleanly re-docking them back into the main window tab strip.

Users can tear off a document by dragging its tab vertically outside the tab strip into open space, or by right-clicking the tab and selecting "Move to New Window". Each detached tab renders its complete document view within a dedicated native window instance with its own header, status indicators, and re-attach controls.

The multi-window system maintains strict state synchronization, robust input routing, and deterministic lifecycle management:
- The primary window tracks only docked (non-detached) views for its active tab state, preventing concurrent rendering of any document.
- Window closure events on detached windows intercept destruction via viewport cancellation (`egui::ViewportCommand::CancelClose`), cleanly prompting to save unsaved changes directly within the active detached window before closing.
- When closing a detached tab completes (via Save, Don't Save, or clean close), an explicit viewport close command (`egui::ViewportCommand::Close`) is dispatched to guarantee deterministic window disposal across both synchronous and asynchronous save flows.
- In-flight text edits, form typing, and queued modifications originating from a detached window commit strictly against the target detached document rather than falling back to the main window's active selection.
- Reloading a replaced file preserves its detachment status across document reinitialization.
- Main window shortcuts and canvas interactions remain active and unblocked while a detached window displays a localized save prompt.
- The dragging tear-off preview badge renders on a dedicated foreground layer, ensuring it remains fully visible regardless of mouse displacement beyond the tab strip bounds.
- Reordering tabs in the tab strip maps visible slot indices back to absolute document view indices, preserving ordering integrity regardless of how many tabs are detached.
- Headless UI control methods and discovery commands accurately advertise multi-viewport availability, reporting detachment as unsupported on WASM runtimes.
- Re-docking is supported via an explicit "Re-attach Tab" button in the detached window header, returning the document immediately to the primary window's tab strip as the active tab.

## User Stories

1. As a user reviewing two different PDF contracts, I want to drag a document tab out of the main window into free space, so that I can place them side by side on my desktop for comparison.
2. As a user editing multi-page documents, I want to tear off a document into a separate window, so that I can easily view and organize pages across two visible documents.
3. As a user working with multiple monitors, I want to move a document tab to a secondary display, so that I can maximize my screen real estate.
4. As a user preferring mouse menus, I want to right-click a document tab and select "Move to New Window", so that I can detach the document without needing to perform a drag gesture.
5. As a user dragging a tab to tear it off, I want to see a floating preview badge that remains visible high above or low below the tab strip, so that I know the detachment gesture is active without the badge clipping away.
6. As a user with only one document open across the entire session, I want the app to disallow detaching the sole tab (suppressing tear-off drag and disabling the menu item), so that I do not accidentally orphan the primary window.
7. As a user detaching the currently active tab, I want focus and active display in the main window to transfer predictably to an adjacent docked tab, so that my primary workspace does not become blank or disrupted.
8. As a user with all documents detached, I want the primary window to display the standard Home/Welcome view, so that I can still open new documents or perform general actions.
9. As a user dragging a tab within the tab strip when other tabs are already detached, I want the drop indicator and reordered position to be exact, so that detached tabs do not displace or misplace my docked tabs.
10. As a user closing a detached window that has no unsaved changes, I want the window to close cleanly and remove the document from the session, so that system resources are immediately freed.
11. As a user closing a detached window that contains unsaved edits or form inputs, I want a confirmation save prompt to appear inside that detached window, so that I am alerted to save my work before the window is destroyed.
12. As a user cancelling a save prompt in a detached window, I want the window to remain open and detached with my unsaved changes intact, so that I do not lose any progress.
13. As a user confirming "Save" in a detached window's close prompt, I want the document to write its changes to disk and close only after the write completes without corrupting the main window's active tab, so that data loss cannot occur.
14. As a user confirming "Don't Save" in a detached window's close prompt, I want the document to discard changes and close cleanly with an explicit window destruction command, so that I can abandon unwanted edits without lingering windows.
15. As a user with a save prompt open inside a detached window, I want to continue using keyboard shortcuts and canvas interactions in the main application window, so that a prompt in one window does not freeze my entire workflow.
16. As a user editing text or typing in form fields inside a detached window, I want those edits to be saved into that specific detached document, so that they never accidentally modify or overwrite the document open in the main window.
17. As a user with a detached document whose file is modified on disk, I want reloading the document to preserve its detached window state, so that the document does not unexpectedly snap back into the main tab strip.
18. As a user closing the primary application window while detached windows are floating, I want the application to evaluate all open documents for unsaved changes and prompt me systematically before terminating the process, so that no work is lost on exit.
19. As a user quitting the application with multiple detached windows open, I want all detached windows to cleanly terminate without leaving orphaned background processes, so that system state remains healthy.
20. As a user with a detached window, I want to click a "Re-attach Tab" button in its header, so that the document returns to the primary window's tab strip as the active tab.
21. As a user on a detached window, I want to see the document title, zoom/page indicator, and an edited marker `(edited)` in the window header, so that I always know which file I am modifying and whether it has pending changes.
22. As a user running PdfCraft on the web via WebAssembly (WASM), I want tear-off gestures and multi-window menu items to be gracefully disabled, and headless discovery commands to report detachment as disabled, so that the web app remains fully functional without failing on unsupported multi-viewport runtime calls.
23. As a keyboard user cycling through open tabs with shortcuts (such as Ctrl+Tab), I want the shortcut to cycle only through docked tabs in the active window, so that detached windows do not unexpectedly take over the main canvas.
24. As a user triggering Undo/Redo (⌘Z / ⇧⌘Z) while focused inside a detached window, I want the history undo/redo actions to apply strictly to that detached document.
25. As a user actively rendering complex pages across multiple detached windows, I want background rendering and thumbnail caching to remain fully active for all open viewports, so that page viewing remains smooth and responsive.
26. As an automated test engineer or AI agent, I want to inspect and control multi-window detachment headlessly, so that feature stability and regressions can be verified programmatically without manual clicking.

## Implementation Decisions

### 1. Viewport Lifecycle and Target Isolation
- Native multiple window support utilizes immediate viewports (`egui::Context::show_viewport_immediate`), with unique viewport IDs derived deterministically from the document identifier (`egui::ViewportId::from_hash_of(("detached-doc", doc_id))`).
- Viewport immediate closures are intercepted during the child UI frame pass. When `child_ui.input(|i| i.viewport().close_requested())` is detected on a child viewport:
  - If the document has unsaved changes, the app issues `child_ui.ctx().send_viewport_cmd_to(viewport_id, egui::ViewportCommand::CancelClose)` to abort premature OS window destruction.
  - The close request is stored as `CloseRequest::Tab(doc_id)`.
  - The modal save confirmation dialog is displayed inside the detached viewport's central UI pass, ensuring visual co-location with the user's close request.
  - Upon user confirmation to discard or completed save, the child window sends an explicit `egui::ViewportCommand::Close` to its viewport ID and completes tab removal via `self.close_tab(idx)`.

### 2. Primary Window Lifecycle and Application Teardown
- In `eframe`, the root window (`ViewportId::ROOT`) controls the main process event loop.
- When the primary window receives a close request while detached windows exist:
  - If a child-scoped close request is already in-flight (`CloseRequest::Tab(id)` for a detached tab), the app directs focus to that child viewport and preserves the pending prompt.
  - Otherwise, the application enters its standard `guard_quit` sequence, evaluating `self.first_dirty()` across all documents in `self.views` (both docked and detached).
  - If dirty documents exist, `CancelClose` is sent to the root viewport, and save confirmation prompts are stepped through sequentially.
  - Once all documents are clean or discarded, `self.allow_quit` is set, closing the root viewport and cleanly terminating all child viewports and worker threads without orphaned processes.

### 3. Active Tab Invariant and Target-Indexed Edit Commits
- The application's active tab index (`self.active`) strictly references only docked, non-detached views (`!v.detached`).
- When a tab is detached, `self.active` shifts to the nearest non-detached tab. If no non-detached tabs remain, `self.active` is set to `None`, which renders the standard Home/Welcome view in the primary window.
- When closing a docked tab in `close_tab` and `close_and_continue`, `self.active` is recalculated strictly over non-detached views.
- Under no circumstances may `self.active` point to an index where `views[i].detached == true`, preventing concurrent dual-rendering of the same document across multiple viewports.
- In-flight text edits, form typing, and queued modifications originating from a detached view must be committed against the target document view index:
  - Define `apply_edit_to(&mut self, view_index: usize, edit: Edit) -> bool`, with `apply_edit` delegating to it via `self.active`.
  - Define `commit_form_typing_in(&mut self, view_index: usize) -> bool`.
  - Define `commit_open_text_edits_in(&mut self, view_index: usize) -> bool`.
  - Call `process_pending_edits_for(&mut self, view_index: usize)` inside the `show_detached_windows` pass for each detached view.
  - Keyboard shortcuts for Undo and Redo pressed inside a detached viewport's UI pass target that detached document's history.

### 4. Preserving Detached State on Document Replacement
- When a document is reloaded or replaced by a newer version from disk, `DocView::document_replaced(&mut self, info: &DocInfo)` in `crates/ui-egui/src/canvas.rs` must copy the detachment flag:
  ```rust
  fresh.detached = self.detached;
  ```
- Reloading a detached document must not clear the detachment state or return the tab to the main window strip.

### 5. Main Window Shortcut and Input Scoping during Detached Prompts
- The application's modal state check (`modal_open(&self) -> bool` in `crates/ui-egui/src/app.rs`) must ignore `CloseRequest::Tab(id)` if `id` belongs to a detached document:
  ```rust
  let close_modal = match self.close_request {
      Some(CloseRequest::Tab(id)) => self.views.iter().find(|v| v.id == id).is_some_and(|v| !v.detached),
      Some(CloseRequest::Quit | CloseRequest::All) => true,
      None => false,
  };
  ```
- Canvas input blocking (`view.objects.set_input_blocked`) in `app.rs` is scoped strictly to the detached view that owns the active prompt, leaving docked views responsive to mouse and keyboard input.

### 6. Foreground Layer Rendering for Drag Tear-Off Badges
- The tear-off boundary threshold requires a minimum vertical displacement (15 points above or below the tab strip bounds) before initiating tear-off state.
- While in tear-off drag mode, the visual drag badge is rendered using the unclipped foreground layer painter:
  ```rust
  let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.scope_id().with("tear_off_badge")));
  ```
- This ensures the badge is never clipped by the scroll area's container rectangle when the pointer moves outside the tab strip bounds.

### 7. Slot-to-View Index Translation for Tab Drag-and-Drop
- The visual tab strip renders only non-detached views, producing a filtered set of slots.
- Drag-and-drop position calculations produce a slot index.
- To prevent index drift caused by preceding detached tabs, the target slot must be mapped back to the absolute view collection index prior to executing tab reordering, using slot-bounded logic:
  ```rust
  let to_view_index = if let Some(&(view_idx, _)) = slots.get(slot_to) {
      view_idx
  } else {
      slots.last().map_or(app.views.len().saturating_sub(1), |&(last_idx, _)| last_idx)
  };
  app.move_tab(from, to_view_index);
  ```

### 8. Sole Tab Detachment Protection
- When only a single document tab exists across the entire session (`self.views.len() == 1`), detaching is disallowed:
  - Vertical dragging outside the tab strip does not initiate tear-off mode and remains clamped as an in-strip tab reorder.
  - The "Move to New Window" option in the tab context menu is disabled with an explanatory tooltip ("Cannot detach the only open document").
- If multiple documents are open and the user detaches the last remaining docked tab, the primary window cleanly displays the Home/Welcome view while the detached window remains operational.

### 9. Headless UI Control Channel & Platform Capability Discovery
- Headless control methods allow test harnesses and automated agents to query detached viewports and trigger detachment or re-attachment programmatically via `PdfCraftControl`.
- The command discovery endpoint (`commands()` in `crates/ui-egui/src/control/host.rs`) advertises detachment and re-attachment as enabled only when supported by the runtime target:
  ```rust
  "enabled": !cfg!(target_arch = "wasm32") && self.views.len() > 1
  ```
  and for re-attachment:
  ```rust
  "enabled": !cfg!(target_arch = "wasm32") && self.views.iter().any(|v| v.detached)
  ```

### 10. Continuous Detached Viewport Rendering & Memory Budgeting
- Render finalization (`finish_render_frame` in `crates/ui-egui/src/app/frame.rs`) treats all open detached viewports as visible alongside the active docked document, preventing cached raster textures and render demands from being prematurely suspended.

### 11. Deterministic Viewport Destruction Across Asynchronous Save Flows
- In `close_and_continue(ctx, index, req)` in `crates/ui-egui/src/editing/close.rs`:
  ```rust
  let detached_id = match req {
      CloseRequest::Tab(id) if self.views.get(index).is_some_and(|v| v.detached) => Some(id),
      _ => None,
  };
  self.close_tab(index);
  if let Some(id) = detached_id {
      ctx.send_viewport_cmd_to(
          egui::ViewportId::from_hash_of(("detached-doc", id)),
          egui::ViewportCommand::Close,
      );
  }
  ```
- Because asynchronous "Save As" flows complete through `close_and_continue`, this guarantees `ViewportCommand::Close` is reliably dispatched whether the user confirmed Save or Don't Save.

## Testing Decisions

### Good Test Principles
- Tests must verify external behavior (window creation, tab presence, active view selection, edit persistence, shortcut responsiveness, and viewport closure commands) rather than private implementation details.
- Integration tests must exercise full interaction cycles: dragging tabs outside bounds, clicking context menu items, triggering keyboard shortcuts, and responding to save prompts.

### Modules Tested
- User interface integration test suite (`crates/ui-egui/tests/it/ui.rs`).
- Headless UI control channel test suite (`crates/ui-egui/tests/it/control.rs`).

### Prior Art
- Multi-document tab ordering tests (`crates/ui-egui/tests/it/ui.rs::dragging_a_tab_to_reorder_moves_it_in_app_views`).
- Multi-window drag and context menu detachment tests (`crates/ui-egui/tests/it/ui.rs::dragging_a_tab_to_open_space_detaches_it_into_its_own_window`, `crates/ui-egui/tests/it/ui.rs::tab_context_menu_can_detach_tab_to_new_window`).
- Headless control channel tests (`crates/ui-egui/tests/it/control.rs::headless_control_channel_tab_detach_and_reattach`).

## Out of Scope

- Split-window view (displaying two viewports of different pages within the *same* document; tracked separately under `view.split-window`).
- Web multi-window viewports on WebAssembly targets (prohibited by browser security models and single-canvas limitations).
- Arbitrary tear-off of toolbars, property inspectors, or side panels.

## Further Notes

- Parity mapping: aligns with Acrobat feature `view.multiple-windows` (tier P1, milestone M14). Because native OS window viewports are specific to desktop GUI runtimes, headless CLI tool coverage is marked `status = "partial"`, with full behavioral evidence verified via UI integration and control-channel test suites.
