# Status

**Read this first every session.** Then the task in
[`execution-plan.md`](execution-plan.md), the relevant part of
[`architecture.md`](architecture.md), and the README of the crate you are touching.

- **Current phase:** D 3.1 (foundation gaps) is done bar review; F 4.1 (font engine, dictionary
  level) is active, with H (fidelity harness) next.
- **Done and merged:** D2 and D3 (#351, tool-argument fuzzing and the root-confinement proof),
  D5 in part (#332, `cos` and `engine` READMEs).
- **Done, awaiting review:** D1 (#339, corpus pin), F1 (#357, exact standard-14 metrics), and this
  plan itself (#223).
- **Next unchecked task:** **F2 — encoding and code-to-Unicode completeness** (§4.1). It needs no
  new decisions and no new dependencies, and it is read from the font dictionary alone.
- **Then:** F3 (deterministic fallback), then H1 (`pdfcraft-testkit`), then D4 (the 96 toolless
  features) and the remaining 11 crate READMEs.
- **Last verified green:** see the baseline below; re-measure before quoting.

## Verified baseline (2026-10-09, `main` at `efb3439`, Windows 10)

Measured, not quoted. **Re-measure before quoting:** `main` moved 123 commits in the 24 hours
before this reading, so a table more than a day old is already wrong.

| Check | Result |
|---|---|
| `cargo xtask parity` | 807 counted features: **420 shipped, 73 partial — 52.0% (56.6% weighted)** |
| Toolless shipped features | **96** (AGENTS.md §3 violation; tracked as D4) |
| Crates | 30 library crates; **11 have no README** (D5) |
| Open PRs / issues | **61 open PRs** (37 of them `CONFLICTING`) / 173 open issues |

Parity by tier: P0 89.6% shipped, P1 54.0%, P2 9.6%, P3 2.3%. Effort-weighted ≈ 30–35%.

Weakest areas by weighted score, which is where the headline 56.6% is actually earned or lost:
I Scan and OCR 23.1%, N Compare/automation/AI 30.0%, K Optimize and standards 34.5%,
J Create and export 39.7%, L Print production 39.7%.

Not re-measured at this commit: `cargo test --workspace`, clippy, the corpus sweep
(`xtask check`), the nightly fuzz job. `xtask parity` and `xtask assets` were run.

> **Compute note.** `cargo test --workspace` locked up the development machine's CPU. Scope every
> build and test to the crates you touched — `cargo test -p <crate> -j 2`, under a `timeout` — and
> leave the workspace sweep to CI, which is what it is for.

### The repository moves fast, and PRs rot

This is the single most important operational fact about this repository, and it now has two
independent readings a day apart:

| Reading | Commits to `main` in the prior 24h | Fate of PRs open the day before |
|---|---|---|
| 2026-10-08 | 47 | **all six** went mergeable → `CONFLICTING` |
| 2026-10-09 | **123** | 37 of 61 open PRs are `CONFLICTING` (61%) |

The five PRs this project opened on 2026-10-08 are a controlled sample of the same effect: two
merged within a day (#332, #351), two stayed green and untouched (#223, #339), and one went
`CONFLICTING` (#357, font metrics). The one that rotted was the one touching code a popular area
was actively changing; the two that survived touched documentation and build tooling. **Conflict
risk tracks how contested the files are, not how large the PR is.**

Three operational consequences, which matter more here than on a slower project:

- **Review small PRs quickly or they die of rebase.** A PR that sits a day needs its author to do
  work again. Latency is a cost paid by contributors, not by us.
- **Ask the author to rebase; do not rebase for them, and never rewrite their branch.** Their
  commits are theirs. See `execution-plan.md` §1.4. Our own branches are ours to rebase, and
  should be rebased the same day `main` moves under them.
- **Prefer uncontested files when there is a choice of equally valuable work.** Tooling, tests,
  docs and the reserved-but-unwritten crates carry near-zero conflict risk; `ui-egui`,
  `crates/fonts` and the i18n catalogues are where several contributors collide at once.

> **Rebasing is a review step, not a mechanical one.** The #357 rebase surfaced a real bug that
> had nothing to do with the conflict: resolving the overlap meant re-reading the merged
> `Family::width`, which was writing items out as `/Times-Italic` while measuring them with
> Times-Roman widths. Read every hunk you resolve, and the auto-merged hunks in the same files
> too — `git` resolved `fonts/src/pdf.rs` silently after 395 lines had changed under it.

## Recent landings worth knowing

**v0.4.0 is tagged.** Since the plan was first drafted: the rename to **PdfCraft** (#174), XFA
dynamic layout (#121) and FormCalc (#301), the non-network half of signatures (#69), signing with
the **Windows certificate store** (#304, which closes the `sign.windows-cert-store` P0 gap),
Measure distance/perimeter/area (#208), print the Pages-panel selection (#296), set-layer-visibility
actions (#292), find text in mixed horizontal/vertical PDFs (#287), ideographic spaces preserved
when rewriting paragraphs (#309), `.p12` files with armour or whitespace (#298), French and
Simplified Chinese catalogs (#291, #311), RTL file names in the tab strip (#285), annotation
appearance box mapping (#277) and `/Widths` handling (#276), Flatpak and AppImage packaging (#275),
and `--root` confinement leak fixes (#164).

Two crates that the architecture table listed as *reserved* now exist: **`measure`** (L4, #208)
and **`xfa`** (L3, #121).

Most of this is community work. The first-party lane should pick what the community is *not*
doing — the foundation items below — rather than racing it on features.

## Blockers

1. ~~**Tooling gates rest on unpinned input.**~~ `xtask corpus` cloned `mozilla/pdf.js` at HEAD
   with no commit pin and no sha256, and corpus tests skipped silently when it was absent — so the
   corpus sweep measured a moving target, against AGENTS.md §2. **Fixed in D1 (#339),** awaiting
   review: the commit and a manifest sha256 are pinned, the checkout is forced to LF so the hash
   matches on Windows and Linux alike, and the corpus test now refuses a directory with no stamp
   instead of passing quietly.
2. ~~**Tool arguments are never fuzzed.**~~ `xtask fuzz` mutated files only, though AGENTS.md §3
   and §4 treat tool arguments as untrusted and they are the MCP attack surface. **Fixed in D2/D3
   (#351, merged):** 19,299 hostile calls across 132 tools, plus 451 path-escape attempts that
   each have to be *refused* by a tool holding a real open document. The first version of that
   confinement test passed with path resolution entirely disabled, which is why it asserts refusal
   rather than comparing a file listing — a worthwhile warning about what a green test proves.
3. **No font engine.** `crates/fonts` reads font dictionaries but never a font *program*; glyph
   outlines exist nowhere outside `hayro`. This blocks M2 (renderer) and M7 (editing existing
   text) — together roughly a third of the remaining effort. Phase F clears it: tranche 4.1 is
   active now, tranche 4.2 follows the harness.
4. **Fidelity is unmeasured.** No side-by-side harness exists. `shipped` means "has at least one
   cited test", and many shipped features rest on exactly one. Treat 52.0% as a coverage map, not
   a quality claim. Phase H exists to fix the measurement.
5. **96 shipped features have no automation tool**, contradicting AGENTS.md §3. Tracked as D4;
   `cargo xtask parity` lists them by name.
6. **The original `plan/` is not present on this machine.** It is gitignored and local-only, so
   the Acrobat observation notes (`plan/acrobat/`) and the ADRs are unavailable here. UI-fidelity
   work has no local reference to compare against; see §Decisions.

## Decisions

Do not guess these; `execution-plan.md` §1 lists what is an owner call.

### Settled

- **The product is PdfCraft** (owner, 2026-10-07; renamed on `main` in #174). The repository is
  `storytold/pdfcraft`. Use PdfCraft in all user-facing text. The local working folder may still
  be named `printcraft`; that is cosmetic.
- **`main` is the only integration branch** (owner, 2026-10-07). No `dev`: the product is still
  being built and a staged promotion pipeline costs more than it returns at this size. `release`
  remains, solely to trigger the installer pipeline. Work branches PR straight into `main`.
- **External PRs are held to safety, security and completeness** before style (owner,
  2026-10-07). The bar is `execution-plan.md` §1.4; provenance is the part a contributor cannot be
  expected to know, and the part that cannot be undone later.

### Parked by the owner

Deferred deliberately, not forgotten. Do not start work that depends on it, and do not re-raise it
as a blocker — raise it again only when the work it gates becomes the priority.

- **Outbound network access.** Held over from session 14 (#63). It now gates only live fetching:
  online timestamp requests and live OCSP/CRL. The non-network half of signatures shipped in #69,
  so the remaining network-free signature work — FieldMDP, lock-after-signing, OS trust stores,
  signature appearances, per-certificate trust — is **not** parked.

### Resolved: standard-14 font data is not gated

Recorded because it was briefly logged as a blocker and should not be re-raised. Both halves were
already permitted and already present:

- **Glyphs:** the 14 Foxit `.pfb` substitute faces under `vendor/hayro-interpret/assets/`, author
  Foxit Software / PDFium Authors — **not Adobe** — BSD-3-Clause, with licence file and SHA-256
  entries in `ATTRIBUTION.toml` that already pass `cargo xtask assets`.
- **Metrics:** AGENTS.md §1.1 permits "the standard-14 font metrics and encoding tables in the PDF
  specification… data, not typefaces", and they ship today as
  `vendor/hayro-interpret/src/font/generated/metrics.rs`, already carrying `adobe_data = true`.
  Writing our own tables from ISO 32000-2 Annex D is inside that existing permission. Retiring
  hayro (M2.8) needs that entry's path reworded, not a new approval.

### Open

- [ ] **Acrobat reference material.** Phase H can measure against the specification and
      independent implementations, but not against Acrobat. If visual parity is to be gated rather
      than eyeballed, we need either access to the existing `plan/acrobat/` notes or a fresh
      black-box observation pass under the AGENTS.md §1.1 rules (synthetic fixtures only, nothing
      committed). H1–H5 do not block on this; H3's goldens pin *our* output against regressions,
      which is a different question from matching Acrobat.
- [ ] **Who owns community PRs.** PhotoCraft runs a separate integration session for incoming PRs,
      merges and `main` breakage. PdfCraft has none, so **61 open PRs — 37 of them already
      conflicting — are currently unowned.** At this volume this is the largest single source of
      wasted contributor effort in the project, and it is growing faster than any feature gap.
      Either a session takes that lane or the feature session covers it under §1.4.

## Conventions reminder

- One task id per commit (`F1: exact standard-14 metrics`). Commit only green states. Branch from
  `main`, PR into `main`, never push to `main` directly.
- Update `parity/acrobat-features.toml` in the same PR as the feature, and say in `notes` what is
  *still* missing. `shipped` is a strong claim — see "Definition of done".
- Add a line to the `ROADMAP.md` log and to `log/devlog.md` at the end of every session, and keep
  ROADMAP's §Honest assessment and §Where we're lacking true.
- Parallel agents: a separate `CARGO_TARGET_DIR` and a separate git worktree each. **Never build
  two worktrees into the same target directory**, and never compare results across them that way.
