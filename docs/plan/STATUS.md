# Status

**Read this first every session.** Then the task in
[`execution-plan.md`](execution-plan.md), the relevant part of
[`architecture.md`](architecture.md), and the README of the crate you are touching.

- **Current phase:** D 3.1 (foundation gaps) and F 4.1 (font engine, dictionary level), with H
  (fidelity harness) alongside.
- **Next unchecked task:** **D1 — pin the pdf.js corpus**. It is a supply-chain hole in the gate
  that every corpus claim rests on, and foundation outranks features.
- **Then:** D2, D3, then F1 (exact standard-14 metrics), F2, F3, then H1 (`pdfcraft-testkit`).
- **Last verified green:** see the baseline below; re-measure before quoting.

## Verified baseline (2026-10-08, `main` at `d801407`, v0.4.0, Windows 10)

Measured, not quoted. **Re-measure before quoting:** `main` moved 47 commits in the 24 hours
before this reading, so a table more than a day old is already wrong.

| Check | Result |
|---|---|
| `cargo xtask parity` | 806 counted features: **413 shipped, 73 partial — 51.2% (55.8% weighted)** |
| Toolless shipped features | **94** (AGENTS.md §3 violation; tracked as D4) |
| Crates | 29 library crates; **13 have no README** (D5) |
| Open PRs / issues | 13 open PRs / ~65 open issues |

Parity by tier: P0 88.8% shipped, P1 53.4%, P2 8.1%, P3 2.3%. Effort-weighted ≈ 30–35%.

Not re-measured at this commit: `cargo test --workspace`, clippy, the corpus sweep
(`xtask check`), the nightly fuzz job. `xtask layers`, `xtask assets` and `xtask parity` were run.

### The repository moves fast, and PRs rot

Between 2026-10-07 and 2026-10-08, 47 commits landed on `main` — mostly community PRs — and
**every one of the six PRs that were open the day before went from mergeable to `CONFLICTING`.**
Two operational consequences, which matter more here than on a slower project:

- **Review small PRs quickly or they die of rebase.** A PR that sits a day needs its author to do
  work again. Latency is a cost paid by contributors, not by us.
- **Ask the author to rebase; do not rebase for them, and never rewrite their branch.** Their
  commits are theirs. See `execution-plan.md` §1.4.

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

1. **Tooling gates rest on unpinned input.** `xtask corpus` clones `mozilla/pdf.js` at HEAD with
   no commit pin and no sha256, and corpus tests skip silently when it is absent — so the corpus
   sweep measures a moving target. AGENTS.md §2 requires pinned, verified fixtures. Tracked as D1.
2. **Tool arguments are never fuzzed.** `xtask fuzz` mutates files only, though AGENTS.md §3 and
   §4 treat tool arguments as untrusted and they are the MCP attack surface. Tracked as D2.
3. **No font engine.** `crates/fonts` reads font dictionaries but never a font *program*; glyph
   outlines exist nowhere outside `hayro`. This blocks M2 (renderer) and M7 (editing existing
   text) — together roughly a third of the remaining effort. Phase F clears it: tranche 4.1 is
   active now, tranche 4.2 follows the harness.
4. **Fidelity is unmeasured.** No side-by-side harness exists. `shipped` means "has at least one
   cited test", and many shipped features rest on exactly one. Treat 51.2% as a coverage map, not
   a quality claim. Phase H exists to fix the measurement.
5. **94 shipped features have no automation tool**, contradicting AGENTS.md §3. Tracked as D4;
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
      merges and `main` breakage. PdfCraft has none, so the six open PRs are currently unowned.
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
