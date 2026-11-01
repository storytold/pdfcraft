# 06: Headless UI Control-Channel Automation & Parity Alignment

**What to build:**
- Headless UI control-channel methods (`PdfCraftControl`) allowing automated test harnesses and AI agents to query detached viewports and trigger tab detachment / re-attachment without manual mouse clicks.
- Update `parity/acrobat-features.toml` evidence citations and tools/commands to provide verified end-to-end evidence for `view.multiple-windows` with headless automation coverage.
- Control-channel integration test verifying programmatic detachment and re-attachment.

**Blocked by:** 03-active-tab-isolation-and-desync-prevention, 04-detached-child-viewport-close-and-unsaved-routing

**Status:** ready-for-agent

- [ ] `PdfCraftControl` in `crates/ui-egui/src/control/host.rs` exposes methods to query detached viewports and detach / attach tabs by `DocId`.
- [ ] Control-channel test in `crates/ui-egui/tests/it/control.rs` verifies programmatic detachment and re-attachment.
- [ ] `parity/acrobat-features.toml` entry for `view.multiple-windows` accurately reflects evidence and status.
