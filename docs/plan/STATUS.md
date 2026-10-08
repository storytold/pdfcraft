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

## Verified baseline (2026-10-07, `main` at `827c61b`, Windows 10)

Measured, not quoted. Re-measure rather than trusting this table if it is more than a few
sessions old.

| Check | Result |
|---|---|
| `cargo xtask parity` | 806 counted features: **411 shipped, 70 partial — 51.0% (55.3% weighted)** |
| Toolless shipped features | **93** (AGENTS.md §3 violation; tracked as D4) |
| Crates | 29 library crates; **13 have no README** (D5) |
| Open PRs / issues | 6 open PRs (all community, all based on `main`) / 65 open issues |

Parity by tier: P0 88.4% shipped, P1 53.4%, P2 7.5%, P3 2.3%. Effort-weighted ≈ 30–35%.

Not re-measured this session: `cargo test --workspace`, clippy, the corpus sweep (`xtask check`),
the nightly fuzz job, and the wasm32 check. Run them before relying on "green".

## Recent landings worth knowing

Since the plan was first drafted: the rename to **PdfCraft** (#174), XFA dynamic layout (#121),
the non-network half of signatures — RFC 3161 timestamps, document timestamps, DSS/VRI and
revocation verification (#69) — Measure distance/perimeter/area (#208), CJK, Czech and pt-BR
catalogs with a Windows display-language fix (#209, #216), `--root` confinement leak fixes (#164),
and theme and picker fixes (#212, #221).

Two crates that the architecture table listed as *reserved* now exist: **`measure`** (L4, #208)
and **`xfa`** (L3, #121).

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
   cited test", and many shipped features rest on exactly one. Treat 51.0% as a coverage map, not
   a quality claim. Phase H exists to fix the measurement.
5. **93 shipped features have no automation tool**, contradicting AGENTS.md §3. Tracked as D4;
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
