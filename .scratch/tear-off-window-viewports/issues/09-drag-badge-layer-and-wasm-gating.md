# 09: Unclipped Foreground Drag Badge & WASM Command Gating

**What to build:**
1. Paint the drag tear-off preview badge on an unclipped foreground tooltip layer using `ui.ctx().layer_painter(...)`, keeping it fully visible across the screen regardless of how far above or below the tab strip the pointer travels.
2. In `crates/ui-egui/src/control/host.rs::commands()`, guard the `enabled` boolean for `ui.detach` and `ui.attach` with `!cfg!(target_arch = "wasm32")`, ensuring headless clients on WASM do not report native window viewports as available.

**Blocked by:** None (can start immediately)

**Status:** done

- [x] In `crates/ui-egui/src/chrome.rs`, paint the tear-off drag badge using `ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.scope_id().with("tear_off_badge")))`.
- [x] In `crates/ui-egui/src/control/host.rs`, update `"enabled"` for `ui.detach` to `!cfg!(target_arch = "wasm32") && self.views.len() > 1` and `ui.attach` to `!cfg!(target_arch = "wasm32") && self.views.iter().any(|v| v.detached)`.
- [x] Add tests verifying unclipped layer badge rendering and WASM command discovery gating.
