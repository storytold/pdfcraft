# Contributing to PdfCraft

Thanks for helping. This page is the short version. The full rules live in
[`CLAUDE.md`](CLAUDE.md), [`AGENTS.md`](AGENTS.md) and the
[pull request template](.github/pull_request_template.md); where they differ from this page, they
win.

## Workflow

1. **Install the GitHub CLI.** `brew install gh` (or your platform's package), then
   `gh auth login`.
2. **Check for existing work.** Search issues and PRs, open and closed, with a few phrasings:
   ```sh
   gh issue list -R storytold/pdfcraft --state all --search "<terms>"
   gh pr list -R storytold/pdfcraft --state all --search "<terms>"
   ```
   If an open PR already covers it, review or build on that PR instead of opening a competing
   one. If a merged fix is already on `main`, the report likely came from an older build.
3. **Fork** if you do not have write access: `gh repo fork --remote`.
4. **Branch** from an up-to-date `main` as `<type>/<slug>` (`feat/`, `fix/`, `docs/`, `chore/`,
   `refactor/`), for example `fix/radio-delete`. `main` is never pushed to directly.
5. **Make the change.** New behaviour needs a test at the right scale: property, unit, crate
   integration or end-to-end. A bug fix needs a regression test: revert the fix and watch the
   test fail before you trust it, because a test nobody has seen fail is not evidence.
   Documentation-only changes need no new test, but still pass the quality gates.
6. **Run the quality gates**, scoped to the crates you touched. A full `--workspace` build can
   lock up a development machine; CI runs it on every PR. Set `CRATE` to the package name of
   each crate you changed (the `name` in its `Cargo.toml`):
   ```sh
   CRATE=pdfcraft-sign
   cargo fmt --check
   timeout 1800 cargo clippy -p "$CRATE" -j 2 --all-targets -- -D warnings
   timeout 1800 cargo test -p "$CRATE" -j 2
   cargo xtask layers
   cargo xtask assets
   ```
   `timeout` stops a build that hangs. Linux has it. On macOS, `brew install coreutils` provides
   it as `gtimeout` (and as `timeout` when Homebrew's `gnubin` directory is on your `PATH`). On
   Windows, `timeout` is a different command: leave the prefix off and watch the build yourself.
7. **Open a PR into `main`** with `gh pr create`, fill in the template's checklists, and add
   `Fixes #123` if there is an issue. Merge only when CI is green.
8. **Rebase on the same day that `main` moves.** `main` takes hundreds of commits a day, so a PR
   that sits goes stale quickly.

## Hard rules

These are the reasons a PR gets rejected.

- **Provenance.** Read [`AGENTS.md`](AGENTS.md) §1 before adding any asset.
  - No assets from Adobe products, and no Adobe icon recreated, traced or imitated.
  - Do not add font files here; new fonts go to
    [storytold/craft-fonts](https://github.com/storytold/craft-fonts). The Latin UI fonts already
    in `assets/fonts/` (Inter, JetBrains Mono, Dancing Script) stay.
  - No copied GPL/AGPL code. MuPDF, Ghostscript, Poppler, veraPDF and DSS run only as external
    oracle processes.
  - Behaviour comes from the specification (ISO 32000-2, the Arlington model) or black-box
    observation, never from reading another implementation's source.
  - Every new asset has an entry in `ATTRIBUTION.toml`.
- **Never crash.** Every PDF, script, MCP call, settings file and keystroke is untrusted input
  ([`AGENTS.md`](AGENTS.md) §4).
  - No `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!`/`unimplemented!` outside tests
    unless provably infallible, with a comment saying why.
  - No indexing or slicing with input-derived positions; use `get`, and slice strings only at
    char boundaries.
  - Checked or saturating arithmetic, capped allocations, bounded recursion.
  - No `unsafe`.
  - Every crash fix gets a small synthetic regression test.
- **Layering.** Nothing below the UI layer depends on egui, winit, eframe or rfd
  (`cargo xtask layers` checks it).
- **Parity and reach.** Update `parity/acrobat-features.toml` with notes on what is still missing
  ("shipped" means exists and tested, not as good as Acrobat), and make new features reachable
  headlessly through an `automation` tool where that applies.
- **Security.** Keep `--root` confinement on every path, including nested tool dispatch. MCP and
  the UI control channel stay opt-in. No new outbound network access.

## Where to start

- Gap 6 in [`docs/gaps.md`](docs/gaps.md), the backlog of wrong results in shipped features. Bug
  fixes are smaller and review faster than new features.
- Tooling, tests and docs rarely conflict with other work. `crates/fonts`, `crates/ui-egui` and
  the i18n catalogues are where several people collide at once.
- [`ROADMAP.md`](ROADMAP.md) and [`docs/gaps.md`](docs/gaps.md) rank the larger work.

## Credit

Contributors appear in About ▸ Contributors as `@username` by default. To be shown by a real or
display name, add only your own `[people.<username>]` entry to `contributors/people.toml` in your
PR (copy the template at the top of the file). Never edit anyone else's entry, and never
hand-edit `contributors/contributors.json`. See [`docs/contributors.md`](docs/contributors.md).

## License

By contributing, you agree that your contributions are licensed under the project's dual
MIT/Apache-2.0 license ([`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE)).
