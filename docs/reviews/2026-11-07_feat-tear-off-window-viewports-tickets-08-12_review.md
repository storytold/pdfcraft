# Code Review: Tear-Off Window Viewports (Tickets 08–12)

**Target:** `240cb15c3...HEAD` (`feat/tear-off-window-viewports-issue-5348`)  
**Date:** 2026-11-07  
**Review Gate Verdict:** `REQUEST_CHANGES`

---

## Executive Summary

The code changes implement the requirements of tickets 08 through 12, adding detachment persistence across reload, foreground layer drag badge rendering, WASM gating, target-indexed edit routing, detached save prompt shortcut isolation, and deterministic viewport close commands.

However, two-axis adversarial evaluation (Standards and Spec) identified key issues that must be addressed prior to landing:
1. **[MAJOR] Global Navigation Shortcut Regression & Focus Leak**: Replacing `egui_wants_keyboard_input()` with `text_edit_focused()` in `canvas/navigation.rs` weakens keyboard input suppression across non-text widgets (sliders, steppers, buttons).
2. **[MAJOR] Missing Viewport Close on Quit / All Prompts**: `close_and_continue` in `editing/close.rs` only sends `ViewportCommand::Close` on `CloseRequest::Tab`, missing detached tabs closed during `Quit` or `Close All`.
3. **[MAJOR] Target-Indexed Undo/Redo in Detached Viewports**: User Story 24 and Design §3 mandate that `⌘Z` / `⇧⌘Z` within a detached window target that detached view's history, whereas undo/redo currently resolves only via `self.active`.
4. **[MAJOR] View Action Routing for Detached Windows**: `process_pending_edits_for(view_index)` routes dialog-based view actions (`InsertFromFileAt`, `ReplacePages`, `Extract`) to helpers hardcoded to `self.active`.
5. **[NIT] Dead Code Shims**: Unused `commit_open_text_edits` and `commit_open_fill_text` annotated with `#[allow(dead_code)]`.
6. **[NIT] Duplicated Modal Logic**: Duplicate `close_modal` match logic in `app.rs`.

---

## Findings by Severity

### 1. [MAJOR] Global Navigation Shortcut Regression & Focus Handling
- **Files**: `crates/ui-egui/src/canvas/navigation.rs:374`
- **Standards Violation**: CLAUDE.md ("Page input", "Review every update... at four zooms")
- **Description**: Replacing `ctx.egui_wants_keyboard_input()` with `ctx.text_edit_focused()` causes non-text focused controls to leak arrow navigation events. Modal button focus in detached save prompts should be handled without compromising non-text keyboard focus on the main canvas.

### 2. [MAJOR] Missing Viewport Close on Quit / All Close Requests
- **Files**: `crates/ui-egui/src/editing/close.rs:123-126`
- **Description**: `close_and_continue` only sends `ViewportCommand::Close` if `matches!(req, CloseRequest::Tab(_))`. During `CloseRequest::Quit` or `CloseRequest::All`, dirty detached tabs saved or discarded in the loop do not receive the explicit close command.
- **Fix**: Check `self.views.get(index).filter(|v| v.detached).map(|v| v.id)` directly instead of matching on `req`.

### 3. [MAJOR] Missing Target-Indexed Undo/Redo in Detached Viewports
- **Files**: `crates/ui-egui/src/app.rs`, `crates/ui-egui/src/editing.rs`
- **Spec Violation**: Design §3 ("Active Tab Invariant and Target-Indexed Edit Commits") and User Story 24 ("Undo/Redo (⌘Z / ⇧⌘Z) while focused inside a detached window apply strictly to that detached document").
- **Fix**: Add `undo_in(view_index)` and `redo_in(view_index)` and capture shortcuts in `show_detached_windows`.

### 4. [MAJOR] View Action Routing in `process_pending_edits_for`
- **Files**: `crates/ui-egui/src/editing.rs:480-488`
- **Description**: `InsertFromFileAt`, `ReplacePages`, and `Extract` ignore `view_index` and invoke methods acting on `self.active`.

### 5. [NIT] Dead Code Compatibility Shims
- **Files**: `crates/ui-egui/src/editing.rs:824-860`
- **Description**: Remove unused wrappers `commit_open_text_edits` and `commit_open_fill_text`.

### 6. [NIT] Duplicated Close Modal Resolution Logic
- **Files**: `crates/ui-egui/src/app.rs`
- **Description**: Deduplicate identical `close_modal` match expressions.
