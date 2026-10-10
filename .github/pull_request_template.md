<!--
Base branch is `main`. Branch naming and the full workflow: docs/plan/execution-plan.md §1.1
Keep a PR to one concern. If describing the diff needs "and" twice, it is two PRs.
Reviewing someone else's PR? The bar is §1.4 — safety, security and completeness before style.
-->

## What and why

<!-- What changed, and the reason. Link the task id (e.g. F1, M2.4, D1) or issue. -->

Closes #

## How it was verified

<!-- Name the tests that would fail without this change, and any manual check. -->

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo xtask layers`
- [ ] `cargo xtask assets`
- [ ] `cargo xtask parity` (if a feature's status changed)
- [ ] Looked at the result in the running app (UI changes only)
- [ ] Proved the new test fails without the fix, by reverting the fix and re-running

## Review zooms

<!-- docs/plan/execution-plan.md §1.2. Tick the levels this change actually warranted, and say
     what each turned up. "Nothing" is a fine answer; "not looked at" is not. -->

- [ ] **Clause** — arithmetic, indexing, slicing, error propagation, boundaries, allocation caps
- [ ] **Function** — totality, edge cases (empty / zero / max / malformed / cyclic), honest return type
- [ ] **File/module** — no second source of truth, nothing dead or duplicated, doc comment still true
- [ ] **Project** — layering, wasm, undo/redo, save fidelity, permissions, tool table, parity entries

## Parity

<!-- Which parity/acrobat-features.toml entries moved, and what their notes now say is still
     missing. Write "none" if this PR changes no feature status. -->

## Safety, security and provenance

- [ ] No `unwrap`/`expect`/`panic!`/`unsafe`/input-derived indexing in non-test code (AGENTS.md §4)
- [ ] Any crash fix has a synthetic regression test that panicked before it
- [ ] `--root` confinement holds on every path, including ones reached by nested tool dispatch
- [ ] No new outbound network request; MCP and the control channel stay opt-in and loopback-only
- [ ] No asset from an Adobe product; no font committed here; any new asset has an `ATTRIBUTION.toml` entry
- [ ] Clean-room respected: behaviour from the spec, public docs or black-box observation only
- [ ] Every user-facing feature is reachable headlessly through an `automation` tool (AGENTS.md §3)
