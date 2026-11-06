# Specification: Tear-Off Document Window Viewports (Issue #5348)

## Problem Statement

When working with multiple PDF documents—such as comparing revisions, referencing source material, or organizing and rearranging pages across different files—users cannot view more than one document simultaneously on screen. Document tabs remain locked inside a single tab strip within the primary application window. Attempting to drag a tab outside the strip or into open screen space does nothing, preventing users from positioning documents side-by-side or distributing them across multiple physical monitors.

Furthermore, an initial prototype implementation revealed critical architectural edge cases:
- Closing a docked document while other documents are detached desynchronizes tab selection, causing the same document view to render concurrently in both the primary window and a detached window, generating dual event conflicts.
- Closing a detached window with unsaved changes fails to intercept window destruction properly: no `CancelClose` command is sent to the child viewport, and the modal prompt displays disconnected on the root window while re-instantiating the child viewport on subsequent frames.
- Reordering tabs via drag-and-drop corrupts the tab order whenever detached tabs precede the drop target due to mismatched index spaces between visible slots and the underlying document views.
- Attempting to detach tabs on WebAssembly (WASM) targets breaks application execution because multi-viewport functionality is not supported by web canvas runtimes.
- Detaching the sole remaining tab leaves the primary window stranded in an undefined home state without clear user guidance.
- Root window termination and save-on-close flows collide with the global close state when multiple windows exist.

## Solution

PdfCraft will support tearing off document tabs into independent native desktop window viewports and cleanly re-docking them back into the main window tab group.

Users can tear off a document by dragging its tab vertically outside the tab strip into open space, or by right-clicking the tab and selecting "Move to New Window". Each detached tab renders its complete document view within a dedicated native window instance with its own header, status indicators, and re-attach controls.

The multi-window system maintains strict state synchronization across all viewports:
- The primary window tracks only docked (non-detached) views for its active tab state (`self.active`), preventing concurrent rendering of any document.
- Root window lifecycle is explicitly coordinated: closing the primary window initiates application shutdown, checking and prompting for unsaved work across all views (both docked and detached).
- Window closure events on detached windows intercept destruction via `egui::ViewportCommand::CancelClose`, cleanly prompting to save unsaved changes directly within the active detached window before closing.
- Saving during close preserves the separation of active views without forcing `self.active` to point to a detached document.
- Tab drag-and-drop slot calculations map accurately back to absolute document view indices, preserving ordering integrity regardless of how many tabs are detached.
- Multi-viewport capabilities are conditionally disabled on WASM targets with clean platform fallbacks.
- Re-docking is supported via an explicit "Re-attach Tab" action in the detached window header, returning the document immediately to the primary window's tab strip.
- Clear behavior is established for single-tab scenarios to maintain application stability.

## User Stories

1. As a user reviewing two different PDF contracts, I want to drag a document tab out of the main window into free space, so that I can place them side by side on my desktop for comparison.
2. As a user editing multi-page documents, I want to tear off a document into a separate window, so that I can easily view and organize pages across two visible documents.
3. As a user working with multiple monitors, I want to move a document tab to a secondary display, so that I can maximize my screen real estate.
4. As a user preferring mouse menus, I want to right-click a document tab and select "Move to New Window", so that I can detach the document without needing to perform a drag gesture.
5. As a user with only one document open, I want the app to disallow detaching the sole tab (suppressing tear-off drag and disabling the menu item), so that I do not accidentally orphan the primary window.
6. As a user detaching the currently active tab, I want focus and active display in the main window to transfer predictably to an adjacent docked tab, so that my primary workspace does not become blank or disrupted.
7. As a user dragging a tab within the tab strip when other tabs are already detached, I want the drop indicator and reordered position to be exact, so that detached tabs do not displace or misplace my docked tabs.
8. As a user closing a detached window that has no unsaved changes, I want the window to close cleanly and remove the document from the session, so that system resources are immediately freed.
9. As a user closing a detached window that contains unsaved edits or form inputs, I want a confirmation save prompt to appear inside that detached window, so that I am alerted to save my work before the window is destroyed.
10. As a user cancelling a save prompt in a detached window, I want the window to remain open and detached with my unsaved changes intact, so that I do not lose any progress.
11. As a user confirming "Save" in a detached window's close prompt, I want the document to write its changes to disk and close only after the write completes without corrupting the main window's active tab, so that data loss cannot occur.
12. As a user confirming "Don't Save" in a detached window's close prompt, I want the document to discard changes and close cleanly, so that I can abandon unwanted edits.
13. As a user closing the primary application window while detached windows are floating, I want the application to evaluate all open documents for unsaved changes and prompt me systematically before terminating the process, so that no work is lost on exit.
14. As a user quitting the application with multiple detached windows open, I want all detached windows to cleanly terminate without leaving orphaned background processes, so that system state remains healthy.
15. As a user with a detached window, I want to click a "Re-attach Tab" button in its header, so that the document returns to the primary window's tab strip as the active tab.
16. As a user on a detached window, I want to see the document title, zoom/page indicator, and an edited marker `(edited)` in the window header, so that I always know which file I am modifying and whether it has pending changes.
17. As a user running PdfCraft on the web via WebAssembly, I want tear-off gestures and multi-window menu items to be gracefully disabled or hidden, so that the web app remains fully functional without crashing due to unsupported multi-viewport runtime calls.
18. As a keyboard user cycling through open tabs with shortcuts (such as Ctrl+Tab), I want the shortcut to cycle only through docked tabs in the active window, so that detached windows do not unexpectedly take over the main canvas.
19. As a user running under Wayland or platform-specific display servers, I want the application to handle window positioning limitations gracefully without crashing, so that desktop environment compliance is maintained.
20. As a user actively rendering complex pages in a detached window, I want background rendering and thumbnail caching to remain fully active for that detached window, so that page viewing remains smooth and responsive.
21. As an automated test engineer or AI agent, I want to inspect and control multi-window detachment headlessly, so that feature stability and regressions can be verified programmatically without manual clicking.

## Implementation Decisions

### 1. Viewport Lifecycle and Target Isolation
- Native multiple window support will utilize immediate viewports (`egui::Context::show_viewport_immediate`), with unique viewport IDs derived deterministically from the document identifier (`egui::ViewportId::from_hash_of(("detached-doc", doc_id))`).
- Viewport immediate closures must be intercepted during the child UI frame pass. When `child_ui.input(|i| i.viewport().close_requested())` is detected on a child viewport:
  - If the document has unsaved changes, the app issues `child_ui.ctx().send_viewport_cmd_to(viewport_id, egui::ViewportCommand::CancelClose)` to abort premature OS window destruction.
  - The close request is flagged per-view on `DocView::close_requested` rather than overwriting global exit flags.
  - The modal save confirmation dialog is displayed inside the detached viewport's central UI pass, ensuring visual co-location with the user's close request.
  - Upon user confirmation to discard or completed save, the child window sends a close command to its viewport and calls `self.close_tab(idx)`.

### 2. Primary Window Lifecycle and Application Teardown
- In `eframe`, the root window (`ViewportId::ROOT`) controls the main process event loop.
- When the primary window receives a close request while detached windows exist:
  - The application enters its standard `guard_quit` sequence, evaluating `self.first_dirty()` across all documents in `self.views` (both docked and detached).
  - If dirty documents exist, `CancelClose` is sent to the root viewport, and save confirmation prompts are stepped through sequentially.
  - Once all documents are clean or discarded, `self.allow_quit` is set, closing the root viewport and cleanly terminating all child viewports and worker threads without orphaned processes.

### 3. Active Tab Invariant and Save-on-Close Separation
- The application's active tab index (`self.active`) must strictly reference only docked, non-detached views (`!v.detached`).
- When a tab is detached, `self.active` shifts to the nearest non-detached tab. If no non-detached tabs remain, `self.active` is set to `None`, which renders the standard Home/Welcome view in the primary window.
- When closing a docked tab in `close_tab` and `close_and_continue`, `self.active` must be recalculated strictly over non-detached views.
- In `resolve_close()`, saving a detached tab must NOT set `self.active = Some(index)`. The document save pipeline (`save_then`) will operate directly on the target document index while preserving `self.active` of the primary window.
- Keyboard tab cycling shortcuts (Ctrl+Tab, ⌘⌥→) must filter out detached views.
- Under no circumstances may `self.active` point to an index where `views[i].detached == true`, preventing concurrent dual-rendering of the same document across multiple viewports.

### 4. Slot-to-View Index Translation for Tab Drag-and-Drop
- The visual tab strip renders only non-detached views, producing a filtered set of slots (`slots`).
- Drag-and-drop position calculations produce a slot index (`slot_to`).
- To prevent index drift caused by preceding detached tabs, `slot_to` must be mapped back to the absolute `views` vector index prior to calling `move_tab`:
  ```rust
  let to_view_index = slots.get(slot_to).map_or(app.views.len() - 1, |(idx, _)| *idx);
  app.move_tab(from, to_view_index);
  ```
- The drag tear-off boundary threshold requires a minimum vertical displacement (15 points above or below the tab strip bounds) before initiating tear-off state. While tearing off, a visual drag badge indicates the document name and detaching action.

### 5. Sole Tab Detachment Rule
- When only a single document tab exists across the application (`self.views.len() == 1`), detaching is disallowed:
  - Vertical dragging outside the tab strip does not initiate tear-off mode and remains clamped as an in-strip tab reorder.
  - The "Move to New Window" option in the tab context menu is disabled with an explanatory tooltip ("Cannot detach the only open document").
- If multiple documents are open and the user detaches the last remaining docked tab, the primary window cleanly displays the Home/Welcome view while the detached window remains operational.

### 6. Detached Window Chrome Specification
Detached windows render a lightweight top header panel and a central document panel:
- **Header Panel**:
  - Document title label (semibold text) and dirty indicator `(edited)` when modified.
  - Compact page navigation and zoom status indicator (`Page X of Y`, zoom level).
  - A right-aligned "Re-attach Tab" button (`tl!("Re-attach Tab")`), which resets `view.detached = false` and sets `self.active = Some(idx)`, bringing the tab back into the primary window.
- **Central Panel**:
  - Hosts `crate::canvas::document_area(self, idx, ui)`, providing full viewing, scrolling, text selection, annotation, and form interaction identical to the primary window.

### 7. Platform and WebAssembly (WASM) Gating
- Native viewports require desktop window manager support (`eframe` desktop backends).
- All multi-viewport rendering, detach actions in context menus, and tear-off drag thresholds will be conditionally compiled under `#[cfg(not(target_arch = "wasm32"))]`.
- On WASM targets, "Move to New Window" is omitted from tab context menus, and vertical drag gestures outside the tab strip cancel cleanly without detaching.
- For Wayland environments where programmatic window positioning is restricted by the compositor, window size and minimum dimensions are supplied via `ViewportBuilder` without enforcing absolute coordinate placement.

### 8. Parity Tracking and Headless Automation Seam
- Parity entry `view.multiple-windows` in `parity/acrobat-features.toml` will be marked `status = "partial"` until dedicated automation commands or UI control-channel tools are implemented.
- The UI control channel (`PdfCraftControl`) will expose methods to detach and re-attach tabs by `DocId`, allowing automated tests and AI agents to simulate window detachment headlessly.

## Testing Decisions

### What Makes a Good Test
- Tests must exercise observable end-to-end user behavior through the UI harness rather than mutating internal boolean flags directly.
- Tests must verify edge cases cold: focus transitions, dirty-state prompts on child viewports, re-docking, and tab reordering consistency.
- Tests must verify that closing an attached tab when another tab is detached never results in dual rendering of the detached view.

### Targeted Modules
- UI integration harness tests (`crates/ui-egui/tests/it/ui.rs`).
- Chrome and tab strip interaction tests (`crates/ui-egui/src/chrome.rs`).
- Document view rendering and cache lifetime tests (`crates/ui-egui/src/canvas/tests.rs`).
- File closing and lifecycle tests (`crates/ui-egui/src/editing/close.rs`).

### Prior Art in Codebase
- Tab drag-reorder testing: `crates/ui-egui/tests/it/ui.rs::dragging_a_tab_moves_it_and_keeps_the_same_document_active`.
- Close prompt and unsaved work handling: `crates/ui-egui/tests/it/ui.rs::closing_the_last_active_tab_selects_the_previous_one_then_home` and `crates/ui-egui/tests/it/editing.rs`.
- Two-document comparison harness: `crates/ui-egui/tests/it/compare.rs`.

## Out of Scope

- Merging two arbitrary detached windows into a second independent multi-tab window group (each detached window hosts a single document tab; grouping only occurs in the primary window).
- Native OS menu bars per detached window (detached windows use a streamlined in-window toolbar with essential controls and re-attach action).
- Cross-process window instances (all windows remain part of the single multi-viewport application process).
- Cross-viewport physical drag-and-drop of OS windows back into the tab strip (re-docking is driven via the explicit "Re-attach Tab" header control).
- Multi-viewport support on WebAssembly/browsers.

## Further Notes

- Relates to Issue #5348: "Ability to drag out a tab to create a new window instance of PdfCraft".
- Aligns with milestone M14 feature `view.multiple-windows`.
- Strict compliance with `CLAUDE.md` and `AGENTS.md` (zero unwrap/panics, strict checked arithmetic, and clean-room implementation).
