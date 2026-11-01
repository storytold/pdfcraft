# Code Review Report: feat-tear-off-window-viewports-issue-5348

- **Date**: 2026-11-01
- **Base Ref**: `c735fb7da55264e7cd84eed0688ec0cec5e628d4` (`storytold/main`)
- **HEAD**: `0e339829b22327c672375d5b9ed7ae17c161b088`
- **Spec Source**: Issue #5348 (`[Feature Request] [PdfCraft] Ability to drag out a tab to create a new window instance of PdfCraft`) & `parity/acrobat-features.toml` (`view.multiple-windows`)
- **Status**: CHANGES REQUIRED

## Executive Summary

| Axis | Total Findings | Blockers | Majors | Nits | Worst Finding |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Standards** | 4 | 0 | 3 | 1 | Unguarded viewport invocation on WASM targets and mismatched tab slot/view index domains |
| **Spec & Architecture** | 5 | 2 | 2 | 1 | Active tab desynchronization causing concurrent duplicate rendering, and broken child viewport close interception for dirty documents |

---

## Standards Review

### Documented Standard Violations

1. **[MAJOR] WASM & Platform Compatibility** ([`CLAUDE.md`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/CLAUDE.md): "PdfCraft... runs natively on macOS, Windows, Linux and FreeBSD, and on the web via WASM")
   - References: [`crates/ui-egui/src/app.rs#L666-L692`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/app.rs#L666-L692), [`crates/ui-egui/src/chrome.rs#L172-L175`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/chrome.rs#L172-L175)
   - Issue: `ctx.show_viewport_immediate(...)` and tab detach actions (`Move to New Window`, drag tear-off) lack `#[cfg(not(target_arch = "wasm32"))]` guards. Web browsers and WASM eframe runtimes do not support native immediate viewports; detaching sets `v.detached = true`, hiding the tab from the strip while failing to spawn an OS window, stranding the document.
   - Fix: Guard viewport detachment actions and viewport rendering with `#[cfg(not(target_arch = "wasm32"))]`.

2. **[MAJOR] Headless Automation** ([`AGENTS.md §3`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/AGENTS.md): "Everything is reachable headlessly... A feature is not done until its tool exists and has an end-to-end test")
   - Reference: [`parity/acrobat-features.toml#L1235-L1249`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/parity/acrobat-features.toml#L1235-L1249)
   - Issue: `view.multiple-windows` is promoted to `status = "shipped"` with `commands = []` and `tools = []`. No engine/automation tool or control-channel endpoint was implemented.
   - Fix: Keep status as `partial` or implement the corresponding headless/control channel tool before claiming `shipped`.

### Fowler Code Smells

3. **[MAJOR] Primitive Obsession / Mismatched Index Domains**
   - Reference: [`crates/ui-egui/src/chrome.rs#L306-L310`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/chrome.rs#L306-L310)
   - Quoted hunk:
     ```rust
     let to = drop_index(&slots, from, pos.x);
     if released {
         if close.is_none() && action.is_none() {
             app.move_tab(from, to);
         }
     }
     ```
   - Problem: `drop_index(&slots, ...)` computes an index into `slots` (`0..slots.len()`). Because detached views are omitted from `slots` (`if app.views[i].detached { continue; }`), `to` reflects filtered slot order, but `app.move_tab(from, to)` operates on `app.views` absolute indices. When any tab prior to the drop target is detached, `app.move_tab` drops the tab into an unintended position.
   - Fix: Map `to` through `slots` back to the underlying `app.views` index.

4. **[NIT] Duplicated Code**
   - Reference: [`crates/ui-egui/src/chrome.rs#L273-L323`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/chrome.rs#L273-L323)
   - Quoted hunk:
     ```rust
     if is_tear_off {
         if released { /* close/action checks */ } else { ui.ctx().set_cursor_icon(...); ... }
     } else {
         if released { /* close/action checks */ } else { ui.ctx().set_cursor_icon(...); ... }
     }
     ```
   - Problem: Drag release validation and `set_cursor_icon` branches are duplicated across tear-off and reorder paths.
   - Fix: Unify release validation and common drag state updates prior to branching on `is_tear_off`.

---

## Spec Review (Goldfish & AST Drift Audit)

### 1. [BLOCKER] Active Tab Desynchronization & Concurrent Viewport Mutation
**Spec Requirement:** "Viewport ID Management: Every detached view must be assigned its own distinct viewport ID." / Issue #5348  
**Offending Code:** [`crates/ui-egui/src/app/files.rs#L661-L666`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/app/files.rs#L661-L666) & [`crates/ui-egui/src/editing/close.rs#L104`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/editing/close.rs#L104)
```rust
self.active = match self.active {
    _ if self.views.is_empty() => None,
    Some(a) if a > index => Some(a - 1),
    Some(a) if a >= self.views.len() => Some(self.views.len() - 1),
    other => other,
};
```
```rust
self.active = Some(index);
```
**Problem:** `close_tab` and `resolve_close` update `self.active` without filtering out `v.detached`. Closing an attached tab can point `self.active` directly to a detached tab. When this happens, `canvas::document_area` renders the identical `DocView` simultaneously inside the main window's `CentralPanel` and inside the child viewport, causing double event processing and state desynchronization.  
**Fix:** Ensure `self.active` selects only non-detached views in `close_tab` and `close_and_continue`.

### 2. [BLOCKER] Child Viewport Unsaved-Close Broken & Missing CancelClose
**Spec Requirement:** "Window Closure Behavior: When a detached window is closed, the app must either re-attach the tab to the main window or cleanly close the document—always prompting if there are unsaved changes."  
**Offending Code:** [`crates/ui-egui/src/app.rs#L667-L698`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/app.rs#L667-L698)
```rust
ctx.show_viewport_immediate(viewport_id, builder, |child_ui, _class| {
    if child_ui.input(|i| i.viewport().close_requested()) {
        close = true;
        return;
    }
    ...
});
if reattach { ... } else if close { self.request_close_tab(idx); }
```
**Problem:** No `egui::ViewportCommand::CancelClose` is sent to `viewport_id`. For dirty documents, `request_close_tab` assigns `self.close_request`, displaying the modal on the main window (`dialogs::show(self, &ctx)`) rather than the detached window. Next frame, because `self.views[idx]` remains open and detached, `show_viewport_immediate` re-spawns the child window while the save prompt sits disconnected on the root window.  
**Fix:** Intercept `close_requested` on the child viewport, issue `CancelClose` if dirty, and present the save prompt within the target window.

### 3. [MAJOR] Missing Platform Guard for WASM
**Spec Requirement:** "Platform Reality Check: ... WASM does not support multiple viewports. Document verified platforms."  
**Offending Code:** [`crates/ui-egui/src/chrome.rs#L172-L175`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/chrome.rs#L172-L175) & [`crates/ui-egui/src/app.rs#L647`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/app.rs#L647)
```rust
if ui.button(tl!("Move to New Window")).clicked() {
    action = Some(crate::tab_context::Action::Detach(id));
```
**Problem:** Tear-off drag detection, tab context menu items, and `show_detached_windows` lack `#[cfg(not(target_arch = "wasm32"))]` guards. Calling multi-viewport functions in web targets causes runtime failures.  
**Fix:** Gate multi-window capabilities behind `#[cfg(not(target_arch = "wasm32"))]`.

### 4. [MAJOR] Missing Drag Re-Docking & Incomplete Test Suite
**Spec Requirement:** "Re-docking / dragging a detached window back into the main tab group." and Author Action Items 2, 3, 4.  
**Offending Code:** [`crates/ui-egui/src/app.rs#L680-L684`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/app.rs#L680-L684)  
**Problem:** Spec-defined drag-docking back into the tab strip is omitted; only a button exists. Additionally, required tests for detaching the sole tab, closing detached tabs with unsaved changes, and re-docking were not added to `crates/ui-egui/tests/it/ui.rs`.  
**Fix:** Implement drag re-docking or document deviation, and provide missing integration tests.

### 5. [NIT] Sole Tab Detachment Unguarded
**Spec Requirement:** "Detaching the sole/last remaining tab: Establishing clear behavior (refused or handled)."  
**Offending Code:** [`crates/ui-egui/src/app.rs#L283-L290`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/app.rs#L283-L290)  
**Problem:** Detaching the last remaining tab clears `self.active`, abruptly sending the primary window back to the Home screen while leaving an orphaned child viewport.  
**Fix:** Disallow detaching the last remaining tab or explicitly handle the primary window empty state.

---

## Actionable Remediation Checklist

- [ ] **Fix active tab desynchronization** in [`close_tab`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/app/files.rs#L661-L666) and [`resolve_close`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/editing/close.rs#L104) to ensure `self.active` only targets non-detached tabs (`!v.detached`), preventing concurrent duplicate rendering in both windows.
- [ ] **Fix detached window close flow**: Send `egui::ViewportCommand::CancelClose` to the child viewport when dirty, route modal save prompts to the active viewport, and prevent re-spawning closed viewports.
- [ ] **Add WASM target guards**: Gate multi-viewport logic (`show_detached_windows`, "Move to New Window" context menu item, and drag tear-off) behind `#[cfg(not(target_arch = "wasm32"))]`.
- [ ] **Fix slot-to-view index mapping**: In [`crates/ui-egui/src/chrome.rs#L306`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/src/chrome.rs#L306), translate `to` from `slots` back to the underlying `app.views` index.
- [ ] **Revert or adjust parity status**: Keep `view.multiple-windows` as `partial` in [`parity/acrobat-features.toml`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/parity/acrobat-features.toml#L1238) until a headless tool or control-channel endpoint exists per AGENTS.md §3.
- [ ] **Implement missing tests** in [`crates/ui-egui/tests/it/ui.rs`](file:///media/saifallah/DATA/desktop_backup/Tech/Projects/pdf_craft_project/pdfcraft/crates/ui-egui/tests/it/ui.rs):
  - Detaching the sole / last remaining tab behavior.
  - Closing a detached window containing unsaved changes.
  - Focus transfer predictability when detaching the active tab.
  - Re-docking / dragging back into the main tab group.
