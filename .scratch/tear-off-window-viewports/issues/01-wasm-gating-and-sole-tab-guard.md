# 01: WASM Compilation Gating & Sole-Tab Detachment Guard

**What to build:**
- Gate all multi-viewport rendering, drag tear-offs, and context menu items behind `#[cfg(not(target_arch = "wasm32"))]`, ensuring web builds compile cleanly without unsupported runtime calls.
- Suppress tab detachment when only a single open document exists (`views.len() == 1`), disabling the context menu item with an explanatory tooltip and constraining vertical drag gestures to in-strip reordering.
- Provide end-to-end UI harness test verifying single-tab drag constraint and disabled context menu state.

**Blocked by:** None (can start immediately).

**Status:** closed

- [x] All multi-viewport methods and view calls in `crates/ui-egui/src/app.rs` and `crates/ui-egui/src/chrome.rs` are gated behind `#[cfg(not(target_arch = "wasm32"))]`.
- [x] Context menu "Move to New Window" is omitted on WASM and disabled with a tooltip if `app.views.len() <= 1`.
- [x] Vertical drag gestures do not enter tear-off mode if `app.views.len() <= 1`.
- [x] UI harness tests in `crates/ui-egui/tests/it/ui.rs` verify sole-tab detachment prevention and menu state.
