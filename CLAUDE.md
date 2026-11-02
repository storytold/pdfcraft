# PdfCraft — instructions for agents

PdfCraft is a clean-room, open-source, Rust-native PDF application targeting Adobe Acrobat Pro parity. It runs natively on macOS, Windows, Linux and FreeBSD, and on the web via WASM. It is the sibling of `../photocraft` (a Photoshop-class editor) and follows the same conventions.

## Start every session here
1. Read `plan/STATUS.md`: the current milestone, the next unchecked task and any blockers. `ROADMAP.md` is the one-page summary (stage, numbers, progress log) and `docs/roadmap.md` the milestone table; at the end of every session update the milestone row and add a line to the ROADMAP.md progress log.
   - Read `docs/gaps.md` (the ranked gaps: Acrobat interop, the open-issue backlog, the renderer and a fidelity harness, editing existing content, Pro workflows, 1.0 polish) and `docs/target-app-parity.md` (the numbers and how they are measured) before choosing work. Prefer the gaps over new P2/P3 features, and keep both documents true when things change (`docs/` progress docs follow craftrules `standards/progress-docs.md`: status line, revision history).
   - "Shipped" in `parity/` means "exists and tested", not "as good as Acrobat". Don't mark a feature shipped on a generic test, and say in its notes what is still missing.
2. Read that task in `plan/execution-plan.md` §3, the relevant section of `plan/architecture.md`, and the README of the crate you're touching.
3. Follow the **autonomous operation protocol** in `plan/execution-plan.md` §7 (orient → plan → implement + test → verify → record → commit). Don't stop to ask unless §7 lists the decision as the user's.

`plan/` is gitignored (local-only, like PhotoCraft). The machine-readable parity checklist lives in `parity/` (committed).

## Non-negotiables
- **Assets: read `AGENTS.md` §1 before adding or showing any icon, image, font or document.** No assets from Adobe products, ever. Only openly licensed or contributor-original assets are allowed, each with an entry in `ATTRIBUTION.toml`. `cargo xtask assets` enforces this. `AGENTS.md` overrides this file.
- **Fonts live in [storytold/craft-fonts](https://github.com/storytold/craft-fonts).** Never commit font files here (`AGENTS.md` §1.4; team members: [craftrules `standards/fonts.md`](https://github.com/storytold/craftrules/blob/main/standards/fonts.md), internal). It is the optional build input `CRAFT_FONTS_DIR`: `git clone https://github.com/storytold/craft-fonts ../craft-fonts && CRAFT_FONTS_DIR=../craft-fonts cargo test --workspace` embeds the Japanese fonts (UI fallback, Japanese text in edited PDFs) and runs their tests, which otherwise skip. Code using `pdfcraft_fonts::CRAFT_FONTS` must work when it is empty.
- **Clean-room.**
  - Never read, disassemble or copy anything inside the Acrobat bundle (names and listings only). **Never open `Contents/Resources/JavaScripts/`.**
  - Behaviour comes from public docs, specs (ISO 32000-2, the Arlington model) and black-box observation (`plan/acrobat/`).
  - Never copy GPL/AGPL code. MuPDF, Ghostscript, Poppler, veraPDF and DSS run only as external oracle processes.
  - See `plan/README.md` §Clean-room and `plan/adr/0001`.
- **Privacy.** When observing Acrobat, use synthetic fixtures only. Never capture the Home view, recent files or account info. Capture by window id (`plan/acrobat/tools/`). Never commit Acrobat outputs, corpus files or personal data.
- **Layering.** Nothing below L7 depends on egui/winit/eframe/rfd (`plan/architecture.md` §3). `cos`/`filters`/`crypt`/`arlington` stay standalone.
- **Fidelity.** The PDF object graph is the model. Preserve unknown data. Saves are incremental unless a full rewrite is required. Never silently drop or repair data without recording it.
- **Never panic.** Every PDF, script, MCP call, settings file and keystroke is untrusted input, and none of it may crash the app or lose the user's work. This outranks feature work. Return `Result` (the crate's error enum) and propagate with `?`, or fall back leniently and record it. In non-test code:
  - No `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!` unless it is provably infallible, with a comment saying why.
  - No indexing or slicing with input-derived positions (use `get`, and slice strings only at char boundaries).
  - Use checked or saturating arithmetic on input-derived numbers, and guard against division by zero and NaN/inf casts.
  - Cap allocations sized by input, and bound recursion with depth limits or seen-sets.
  - Handle lock poisoning.
  - No `unsafe` (`unsafe_code = "forbid"`).
  - Every crash fix gets a synthetic regression test. See `AGENTS.md` §4 (and, for team members, the internal `craftrules/standards/never-crash.md`).
- **Rust only** in the product and build (`xtask`). No handwritten JS/TS.
- **Review every update as you make it, at four zooms.** Not an end-of-task pass — a per-update habit, so a defect never gets built on. **Clause:** checked arithmetic, `get()` not `[i]`, char-boundary slicing, the right error through `?`, `<` against `<=`, the 1-based/0-based seam, capped allocations. **Function:** is it total — name an input that panics; walk empty, zero, max, malformed, cyclic; is the return type honest. **File/module:** two sources of truth, dead or duplicated code, is the public surface still smallest and the doc comment still true. **Project:** layering, wasm, undo/redo, save fidelity, encryption and permissions, a matching `automation` tool or registry entry, and whether a `parity/` status or note just became untrue. Scale the zoom to the change, and fix what a level turns up before widening.
  - Tests mirror the zooms: property → unit → crate integration → end-to-end.
  - **Prove a test fails before the fix by actually reverting the fix.** A test nobody has seen fail is not evidence. This has caught two worthless tests in this repo: one confinement test passed with path resolution switched off entirely, because the tools failed on a nonexistent document id before reaching the path.
  - **Rebasing is a review step, not a mechanical one.** Read every hunk you resolve, and the auto-merged hunks in the files you touched too — `main` moves fast enough that a file can change by hundreds of lines under a branch.
- **External PRs are held to safety, security and completeness before style.** Provenance first (no Adobe-derived asset, no committed font, no copied GPL/AGPL code, behaviour from the spec and not from reading another implementation), then no new panic path or `unsafe`, then `--root` confinement on every path including ones reached by nested dispatch, opt-in-only MCP and control channel, no new outbound network, then a headless `automation` tool, tests at the right zoom and an honest `parity/` entry. Provenance is the part a contributor cannot be expected to know, and the only part that cannot be undone after merge.
- **Quality gates** before every commit: `cargo fmt --check`, clippy with `-D warnings`, the tests, `cargo xtask layers`, `cargo xtask assets`, and the wasm check once `xtask ci` exists (M0).
  - **Scope builds and tests to the crates you touched** — `cargo clippy -p <crate> -j 2 --all-targets` and `cargo test -p <crate> -j 2`, under a `timeout`. A full `--workspace` build has locked up a development machine's CPU; it is CI's job and CI runs it on every PR. Run the workspace sweep locally only when you have a reason to and the machine is free.
- **Commits and PRs:** `main` is the only integration branch and is **never pushed to directly**. Branch from an up-to-date `main` as `<type>/<slug>` (`feat/`, `fix/`, `docs/`, `chore/`, `refactor/`), one task id per commit (e.g. `M1.4: xref stream reader`), commit only green states, and open a PR into `main` using `.github/pull_request_template.md`. Merge only when CI is green.
  - **`main` moves fast enough that PRs rot.** It took 396 commits in the three days to 2026-10-12, and most open PRs are conflicting at any given time. Rebase your own branch the same day `main` moves under it, and ask the author to rebase theirs — never rewrite a contributor's branch. Conflict risk tracks how *contested* the files are, not how large the change is: tooling, tests and docs are nearly free, while `crates/fonts`, `ui-egui` and the i18n catalogues are where several people collide at once.

## Running and looking at the app
- `cargo run -p pdfcraft -- <file.pdf>` opens the desktop app.
- `cargo run -p pdfcraft-cli -- run --script steps.json --root DIR` drives the engine headlessly through the automation tools (`pdfcraft-cli tools` lists them). Use it, together with `page_render`, to check engine changes. `pdfcraft-cli mcp` is the opt-in MCP server (AGENTS.md §3).
- `cargo xtask fuzz --time 300` mutation-fuzzes open/render/edit/save in child processes. Findings land in `fuzz-out/findings/` (git-ignored; never commit corpus-derived files). Turn every real finding into a small synthetic regression test before fixing it.
- `cargo xtask parity [--partial]` reports Acrobat-parity progress from `parity/acrobat-features.toml`. Update the entry when a feature ships.
- `cargo xtask demo-pdf` builds `dist/demo/pdfcraft-showcase.pdf` (needs Chrome) for visual checks.
- For UI work, **look at the result**. Either launch `pdfcraft --control FILE doc.pdf` and use `pdfcraft-cli ui --control FILE screenshot --out x.png` (plus `inspect`, `click`, `key`, `type`, `command`, `set`), or take a headless shot with `cargo run -p pdfcraft-ui-egui --example shot`. Compare against `plan/acrobat/02-ui-ux.md`. Control-channel tests use kittest (`crates/ui-egui/tests/control.rs`).
- Parallel agents: use a separate `CARGO_TARGET_DIR` per agent and separate git worktrees.

## Current bootstrap debt (tracked in STATUS.md)
- `pdfcraft-render` renders through the `hayro` crate directly and inspects documents through `lopdf`. Both get replaced by `cos` / `model` / the DisplayList device (M1–M2, ADR-0004).
